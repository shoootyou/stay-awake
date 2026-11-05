/**
 * @spec-handoff
 * @interface handle_runtime_route(RuntimeRouteInput, Supervisor, EngineBackend, TraySink)
 * @behavior
 *   - Startup follows the checkbox lifecycle for Manual and AlwaysOn modes.
 *   - A schedule-enabled startup starts only inside its selected window.
 *   - Legacy Scheduled uses the Manual startup behavior without changing persisted config.
 *
 * @edge-cases
 *   - Every startup publishes exactly one truthful tray status.
 */
// Stay Awake is the Tauri application entry point.
mod config;
mod engine;
mod i18n;
#[cfg(desktop)]
mod owner;
mod platform;
mod runtime;
mod schedule;
mod wifi;

#[cfg(test)]
use config::AppMode;
use config::{AppConfig, JiggleMode, Profile};
use engine::Engine;
use i18n::Bundle as I18nBundle;
use platform::PermissionChecker;
use std::sync::{Arc, Mutex};
use tauri::Manager;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrayRoute {
    Startup,
    TrayToggle,
    Hotkey,
    IpcToggle,
    Tick,
    WifiChanged { active: bool },
    ConfigChanged { policy_changed: bool },
    ProfileLoaded { policy_changed: bool },
    LanguageChanged,
    Resume,
    Quit,
}

impl TrayRoute {
    pub(crate) fn lifecycle_event(self) -> Option<runtime::LifecycleEvent> {
        match self {
            Self::Startup => Some(runtime::LifecycleEvent::Startup),
            Self::TrayToggle | Self::Hotkey | Self::IpcToggle => {
                Some(runtime::LifecycleEvent::TrayToggle)
            }
            Self::Tick => Some(runtime::LifecycleEvent::Tick),
            Self::WifiChanged { active } => Some(runtime::LifecycleEvent::WifiChanged { active }),
            Self::ConfigChanged { policy_changed } | Self::ProfileLoaded { policy_changed } => {
                Some(runtime::LifecycleEvent::ConfigChanged { policy_changed })
            }
            Self::LanguageChanged => None,
            Self::Resume => Some(runtime::LifecycleEvent::Resume),
            Self::Quit => Some(runtime::LifecycleEvent::Quit),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrayPresentation {
    pub(crate) status: runtime::TrayStatus,
    pub(crate) menu_text: String,
    pub(crate) tooltip: String,
    pub(crate) icon: runtime::TrayIcon,
}

pub(crate) struct PresentationInput<'a> {
    pub(crate) snapshot: &'a runtime::RuntimeSnapshot,
    pub(crate) bundle: &'a i18n::Bundle,
    pub(crate) locale: &'a str,
    pub(crate) clock: &'a dyn schedule::Clock,
}

pub(crate) fn build_tray_presentation(input: PresentationInput<'_>) -> TrayPresentation {
    let status = input.snapshot.status;
    let mut tooltip = i18n::t(input.bundle, status.tooltip_key());
    if matches!(
        status,
        runtime::TrayStatus::Waiting
            | runtime::TrayStatus::Paused
            | runtime::TrayStatus::ActiveOverride
    ) {
        let suffix = match input.snapshot.window.next.as_ref() {
            Some(next) => {
                let mut args = fluent_bundle::FluentArgs::new();
                args.set(
                    "when",
                    i18n::format_next_window(i18n::NextWindowInput {
                        when: input.clock.to_local(next.start),
                        locale: input.locale,
                    }),
                );
                i18n::t_with_args(input.bundle, "tray-next-window", Some(&args))
            }
            None => i18n::t(input.bundle, "tray-no-next-window"),
        };
        tooltip.push_str(" (");
        tooltip.push_str(&suffix);
        tooltip.push(')');
    }
    TrayPresentation {
        status,
        menu_text: i18n::t(input.bundle, status.menu_key()),
        tooltip,
        icon: status.icon(),
    }
}

pub(crate) trait TraySink {
    fn publish(&mut self, route: TrayRoute, presentation: &TrayPresentation) -> Result<(), String>;

    /// Reports an owner operation error when no caller is waiting for a reply.
    fn report_error(&mut self, error: &str) {
        log::error!("runtime operation failed: {error}");
    }
}

#[cfg(desktop)]
pub(crate) type PostSpawnStep<'a> = Box<dyn FnOnce() -> Result<(), String> + 'a>;

/// Runs post-spawn setup steps and shuts the owner down before returning the first failure.
#[cfg(desktop)]
pub(crate) fn run_post_spawn_steps<'a, I, S>(
    runtime: &owner::RuntimeHandle,
    steps: I,
    shutdown: S,
) -> Result<(), String>
where
    I: IntoIterator<Item = PostSpawnStep<'a>>,
    S: FnOnce(&owner::RuntimeHandle) -> Result<(), String>,
{
    for step in steps {
        if let Err(error) = step() {
            return match shutdown(runtime) {
                Ok(()) => Err(error),
                Err(cleanup_error) => {
                    Err(format!("{error}; owner shutdown failed: {cleanup_error}"))
                }
            };
        }
    }
    Ok(())
}

#[cfg(desktop)]
pub(crate) struct TrayImages {
    pub(crate) active: tauri::image::Image<'static>,
    pub(crate) inactive: tauri::image::Image<'static>,
    pub(crate) waiting: tauri::image::Image<'static>,
}

#[cfg(desktop)]
pub(crate) struct NativeTrayTargets {
    pub(crate) tray: tauri::tray::TrayIcon<tauri::Wry>,
    pub(crate) status_item: tauri::menu::MenuItem<tauri::Wry>,
    pub(crate) images: TrayImages,
}

#[cfg(desktop)]
pub(crate) trait TrayPaintTargets: Send + Sync {
    fn paint(&self, presentation: &TrayPresentation) -> Result<(), String>;
}

#[cfg(desktop)]
struct PaintMailbox {
    pending: Option<(TrayRoute, TrayPresentation)>,
    last_requested: Option<TrayPresentation>,
    scheduled: bool,
}

#[cfg(desktop)]
pub(crate) enum PaintQueueDecision {
    Coalesced,
    Enqueue,
}

#[cfg(desktop)]
pub(crate) fn queue_paint(
    mailbox: &mut PaintMailbox,
    route: TrayRoute,
    presentation: TrayPresentation,
) -> PaintQueueDecision {
    if route == TrayRoute::Tick && mailbox.last_requested.as_ref() == Some(&presentation) {
        return PaintQueueDecision::Coalesced;
    }
    mailbox.last_requested = Some(presentation.clone());
    mailbox.pending = Some((route, presentation));
    if mailbox.scheduled {
        PaintQueueDecision::Coalesced
    } else {
        mailbox.scheduled = true;
        PaintQueueDecision::Enqueue
    }
}

#[cfg(desktop)]
pub(crate) fn take_pending_paint(
    mailbox: &mut PaintMailbox,
) -> Option<(TrayRoute, TrayPresentation)> {
    mailbox.scheduled = false;
    mailbox.pending.take()
}

#[cfg(desktop)]
pub(crate) trait UiExecutor: Send + Sync {
    fn run_on_main_thread(&self, task: Box<dyn FnOnce() + Send>) -> Result<(), String>;
}

#[cfg(desktop)]
struct TauriUiExecutor(tauri::AppHandle);

#[cfg(desktop)]
impl UiExecutor for TauriUiExecutor {
    fn run_on_main_thread(&self, task: Box<dyn FnOnce() + Send>) -> Result<(), String> {
        self.0
            .run_on_main_thread(task)
            .map_err(|error| error.to_string())
    }
}

#[cfg(desktop)]
pub(crate) struct NativeTraySink {
    targets: Arc<dyn TrayPaintTargets>,
    mailbox: Arc<Mutex<PaintMailbox>>,
    executor: Arc<dyn UiExecutor>,
}

#[cfg(desktop)]
impl NativeTraySink {
    pub(crate) fn new(app: tauri::AppHandle, targets: NativeTrayTargets) -> Self {
        Self::new_with_ui(Arc::new(TauriUiExecutor(app)), Arc::new(targets))
    }

    /// Lets crate tests drive mailbox and paint failures without a native event loop.
    pub(crate) fn new_with_ui(
        executor: Arc<dyn UiExecutor>,
        targets: Arc<dyn TrayPaintTargets>,
    ) -> Self {
        Self {
            targets,
            mailbox: Arc::new(Mutex::new(PaintMailbox {
                pending: None,
                last_requested: None,
                scheduled: false,
            })),
            executor,
        }
    }

    pub(crate) fn apply_initial(&self, presentation: &TrayPresentation) -> Result<(), String> {
        let status_item = InitialStatusItem(&*self.targets);
        status_item.set_text(presentation)
    }
}

#[cfg(desktop)]
struct InitialStatusItem<'a>(&'a dyn TrayPaintTargets);

#[cfg(desktop)]
impl InitialStatusItem<'_> {
    fn set_text(&self, presentation: &TrayPresentation) -> Result<(), String> {
        self.0.paint(presentation)
    }
}

#[cfg(desktop)]
impl TrayPaintTargets for NativeTrayTargets {
    fn paint(&self, presentation: &TrayPresentation) -> Result<(), String> {
        let image = match presentation.icon {
            runtime::TrayIcon::Active => self.images.active.clone(),
            runtime::TrayIcon::Inactive => self.images.inactive.clone(),
            runtime::TrayIcon::Waiting => self.images.waiting.clone(),
        };
        let mut errors = Vec::new();
        if let Err(error) = self.status_item.set_text(&presentation.menu_text) {
            errors.push(error.to_string());
        }
        if let Err(error) = self.tray.set_tooltip(Some(&presentation.tooltip)) {
            errors.push(error.to_string());
        }
        #[cfg(target_os = "macos")]
        if let Err(error) = self.tray.set_icon_with_as_template(Some(image), true) {
            errors.push(error.to_string());
        }
        #[cfg(not(target_os = "macos"))]
        if let Err(error) = self.tray.set_icon(Some(image)) {
            errors.push(error.to_string());
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

#[cfg(desktop)]
impl TraySink for NativeTraySink {
    fn publish(&mut self, route: TrayRoute, presentation: &TrayPresentation) -> Result<(), String> {
        let decision = {
            let mut mailbox = self
                .mailbox
                .lock()
                .map_err(|_| "paint mailbox lock poisoned".to_owned())?;
            let _last_requested = &mailbox.last_requested;
            queue_paint(&mut mailbox, route, presentation.clone())
        };
        if matches!(decision, PaintQueueDecision::Coalesced) {
            return Ok(());
        }
        let mailbox = Arc::clone(&self.mailbox);
        let targets = Arc::clone(&self.targets);
        if let Err(error) = self.executor.run_on_main_thread(Box::new(move || {
            let frame = match mailbox.lock() {
                Ok(mut mailbox) => take_pending_paint(&mut mailbox),
                Err(_) => {
                    log::error!("paint mailbox lock poisoned");
                    None
                }
            };
            if let Some((_route, presentation)) = frame {
                if let Err(error) = targets.paint(&presentation) {
                    log::error!("tray paint failed: {error}");
                }
            }
        })) {
            if let Ok(mut mailbox) = self.mailbox.lock() {
                mailbox.scheduled = false;
            }
            return Err(error);
        }
        Ok(())
    }
}

pub(crate) struct InitialSnapshotInput<'a> {
    pub(crate) config: &'a config::AppConfig,
    pub(crate) clock: &'a dyn schedule::Clock,
}

pub(crate) fn initial_runtime_snapshot(
    input: InitialSnapshotInput<'_>,
) -> Result<runtime::RuntimeSnapshot, String> {
    let schedule = schedule::Schedule::from_config(input.config)?;
    let window = schedule::evaluate_schedule(schedule::ScheduleInput {
        schedule: &schedule,
        at: input.clock.now(),
        clock: input.clock,
    })?;
    let override_state = runtime::normalize_override(runtime::NormalizeInput {
        mode: &input.config.mode,
        schedule_enabled: schedule.enabled,
        window: &window,
        override_state: &runtime::OverrideState::None,
        event: runtime::LifecycleEvent::Tick,
    });
    let status = runtime::derive_tray_status(runtime::TrayInput {
        mode: &input.config.mode,
        engine_active: false,
        schedule_enabled: schedule.enabled,
        window: &window,
        override_state: &override_state,
    });
    Ok(runtime::RuntimeSnapshot {
        engine_active: false,
        override_state,
        window,
        status,
    })
}

#[cfg(desktop)]
pub(crate) fn request_app_exit(app: &tauri::AppHandle, code: i32) -> Result<(), String> {
    use owner::SHUTDOWN_TIMEOUT;

    let runtime = app
        .try_state::<owner::RuntimeHandle>()
        .ok_or_else(|| "runtime is not initialized".to_owned())?
        .inner()
        .clone();
    if !runtime.claim_exit_coordinator() {
        return Ok(());
    }
    let app = app.clone();
    let release_claim = runtime.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("stay-awake-exit".to_owned())
        .spawn(move || {
            let result = runtime.shutdown(SHUTDOWN_TIMEOUT);
            runtime.release_exit_coordinator();
            match result {
                Ok(()) => app.exit(code),
                Err(error) => log::error!("Runtime shutdown did not complete: {error}"),
            }
        })
    {
        release_claim.release_exit_coordinator();
        return Err(format!("failed to start exit coordinator: {error}"));
    }
    Ok(())
}

pub(crate) struct TrayRefreshInput<'a> {
    pub(crate) route: TrayRoute,
    pub(crate) presentation: &'a TrayPresentation,
}

pub(crate) fn refresh_tray_status(
    input: TrayRefreshInput<'_>,
    sink: &mut dyn TraySink,
) -> Result<(), String> {
    sink.publish(input.route, input.presentation)
}

pub(crate) struct RuntimeRouteInput<'a> {
    pub(crate) route: TrayRoute,
    pub(crate) config: &'a config::AppConfig,
    pub(crate) clock: &'a dyn schedule::Clock,
    pub(crate) bundle: &'a i18n::Bundle,
}

pub(crate) fn handle_runtime_route(
    input: RuntimeRouteInput<'_>,
    supervisor: &mut runtime::Supervisor,
    backend: &mut dyn runtime::EngineBackend,
    sink: &mut dyn TraySink,
) -> Result<runtime::RuntimeSnapshot, String> {
    let lifecycle_result = match input.route.lifecycle_event() {
        Some(event) => supervisor.reconcile(
            runtime::ReconcileInput {
                config: input.config,
                clock: input.clock,
                event,
            },
            backend,
        ),
        None => supervisor
            .snapshot()
            .cloned()
            .ok_or_else(|| "tray status is not initialized".to_owned()),
    };
    let snapshot = match lifecycle_result.as_ref() {
        Ok(snapshot) => snapshot.clone(),
        Err(_) => supervisor
            .snapshot()
            .cloned()
            .ok_or_else(|| "runtime reconciliation produced no snapshot".to_owned())?,
    };
    let presentation = build_tray_presentation(PresentationInput {
        snapshot: &snapshot,
        bundle: input.bundle,
        locale: &input.config.language,
        clock: input.clock,
    });
    let publish_result = refresh_tray_status(
        TrayRefreshInput {
            route: input.route,
            presentation: &presentation,
        },
        sink,
    );
    match (lifecycle_result, publish_result) {
        (Ok(_), Ok(())) => Ok(snapshot),
        (Err(lifecycle_error), Ok(())) => Err(lifecycle_error),
        (Ok(_), Err(publish_error)) => Err(publish_error),
        (Err(lifecycle_error), Err(publish_error)) => Err(format!(
            "{lifecycle_error}; tray status publication failed: {publish_error}"
        )),
    }
}

/// Return the current configuration to the frontend.
#[tauri::command]
fn get_config(config: tauri::State<'_, Arc<Mutex<AppConfig>>>) -> Result<AppConfig, String> {
    let cfg = config
        .lock()
        .map_err(|e| format!("Config lock poisoned: {}", e))?;
    Ok(cfg.clone())
}

#[cfg(test)]
mod tray_refresh_contract_tests {
    /*
     * @spec-handoff
     * @interface TrayRoute::lifecycle_event; build_tray_presentation; refresh_tray_status
     * @behavior
     *   - Every runtime route maps to its prescribed lifecycle event and publishes once.
     *   - Presentations use the current bundle for menu text, tooltip, and state icon.
     * @edge-cases
     *   - WiFi has only Active or Inactive states; failed backend work publishes actual state.
     *   - A language change redraws from the replacement bundle without a lifecycle action.
     */
    use super::*;
    use chrono::{DateTime, FixedOffset, LocalResult, NaiveDateTime, TimeZone, Utc};

    #[derive(Default)]
    struct RecordingSink {
        calls: Vec<(TrayRoute, TrayPresentation)>,
    }

    impl TraySink for RecordingSink {
        fn publish(
            &mut self,
            route: TrayRoute,
            presentation: &TrayPresentation,
        ) -> Result<(), String> {
            self.calls.push((route, presentation.clone()));
            Ok(())
        }
    }

    struct FixedClock(DateTime<Utc>);

    impl schedule::Clock for FixedClock {
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
    struct RecordingBackend {
        active: bool,
        fail_start: bool,
        start_calls: usize,
        stop_calls: usize,
    }

    impl runtime::EngineBackend for RecordingBackend {
        fn start(&mut self) -> Result<(), String> {
            self.start_calls += 1;
            if self.fail_start {
                return Err("start failed".to_owned());
            }
            self.active = true;
            Ok(())
        }

        fn stop(&mut self) -> Result<(), String> {
            self.stop_calls += 1;
            self.active = false;
            Ok(())
        }

        fn is_active(&self) -> bool {
            self.active
        }

        fn set_jiggle_policy(&mut self, _: runtime::JigglePolicy) {}
    }

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, 2, hour, 0, 0)
            .single()
            .expect("fixed instant exists")
    }

    fn snapshot(status: runtime::TrayStatus) -> runtime::RuntimeSnapshot {
        let window = schedule::ScheduleEvaluation {
            at: at(10),
            current: None,
            next: None,
        };
        let override_state = runtime::normalize_override(runtime::NormalizeInput {
            mode: &AppMode::Manual,
            schedule_enabled: false,
            window: &window,
            override_state: &runtime::OverrideState::None,
            event: runtime::LifecycleEvent::Tick,
        });
        runtime::RuntimeSnapshot {
            engine_active: matches!(
                status,
                runtime::TrayStatus::Active | runtime::TrayStatus::ActiveOverride
            ),
            override_state,
            window,
            status,
        }
    }

    #[cfg(desktop)]
    fn presentation(text: &str) -> TrayPresentation {
        TrayPresentation {
            status: runtime::TrayStatus::Active,
            menu_text: text.to_owned(),
            tooltip: text.to_owned(),
            icon: runtime::TrayIcon::Active,
        }
    }

    #[cfg(desktop)]
    fn mailbox() -> PaintMailbox {
        PaintMailbox {
            pending: None,
            last_requested: None,
            scheduled: false,
        }
    }

    #[cfg(desktop)]
    #[test]
    fn paint_mailbox_keeps_only_the_latest_frame_and_allows_a_follow_up_paint() {
        let mut mailbox = mailbox();
        assert!(matches!(
            queue_paint(&mut mailbox, TrayRoute::Tick, presentation("A")),
            PaintQueueDecision::Enqueue
        ));
        assert!(matches!(
            queue_paint(&mut mailbox, TrayRoute::Tick, presentation("B")),
            PaintQueueDecision::Coalesced
        ));
        let (_, first) = take_pending_paint(&mut mailbox).expect("the newest queued frame exists");
        assert_eq!(first.menu_text, "B");

        assert!(matches!(
            queue_paint(&mut mailbox, TrayRoute::LanguageChanged, presentation("C")),
            PaintQueueDecision::Enqueue
        ));
        let (_, second) =
            take_pending_paint(&mut mailbox).expect("a frame queued during paint remains pending");
        assert_eq!(second.menu_text, "C");
    }

    #[cfg(desktop)]
    #[test]
    fn paint_mailbox_coalesces_unchanged_ticks_but_repaints_explicit_routes() {
        let mut mailbox = mailbox();
        let frame = presentation("active");
        assert!(matches!(
            queue_paint(&mut mailbox, TrayRoute::Tick, frame.clone()),
            PaintQueueDecision::Enqueue
        ));
        assert!(matches!(
            queue_paint(&mut mailbox, TrayRoute::Tick, frame.clone()),
            PaintQueueDecision::Coalesced
        ));
        take_pending_paint(&mut mailbox).expect("initial frame is pending");
        assert!(matches!(
            queue_paint(&mut mailbox, TrayRoute::Resume, frame),
            PaintQueueDecision::Enqueue
        ));
    }

    #[test]
    fn every_tray_route_maps_to_its_prescribed_lifecycle_event() {
        let cases = [
            (TrayRoute::Startup, Some(runtime::LifecycleEvent::Startup)),
            (
                TrayRoute::TrayToggle,
                Some(runtime::LifecycleEvent::TrayToggle),
            ),
            (TrayRoute::Hotkey, Some(runtime::LifecycleEvent::TrayToggle)),
            (
                TrayRoute::IpcToggle,
                Some(runtime::LifecycleEvent::TrayToggle),
            ),
            (TrayRoute::Tick, Some(runtime::LifecycleEvent::Tick)),
            (
                TrayRoute::WifiChanged { active: true },
                Some(runtime::LifecycleEvent::WifiChanged { active: true }),
            ),
            (
                TrayRoute::ConfigChanged {
                    policy_changed: true,
                },
                Some(runtime::LifecycleEvent::ConfigChanged {
                    policy_changed: true,
                }),
            ),
            (
                TrayRoute::ProfileLoaded {
                    policy_changed: false,
                },
                Some(runtime::LifecycleEvent::ConfigChanged {
                    policy_changed: false,
                }),
            ),
            (TrayRoute::LanguageChanged, None),
            (TrayRoute::Resume, Some(runtime::LifecycleEvent::Resume)),
            (TrayRoute::Quit, Some(runtime::LifecycleEvent::Quit)),
        ];
        for (route, expected) in cases {
            assert_eq!(route.lifecycle_event(), expected, "{route:?}");
        }
    }

    #[test]
    fn builds_and_publishes_the_english_presentation_for_every_status() {
        let clock = FixedClock(at(10));
        let bundle = i18n::create_bundle("en");
        let cases = [
            (
                runtime::TrayStatus::Active,
                "Active",
                "Stay Awake — Active",
                runtime::TrayIcon::Active,
            ),
            (
                runtime::TrayStatus::Inactive,
                "Start",
                "Stay Awake — Inactive",
                runtime::TrayIcon::Inactive,
            ),
            (
                runtime::TrayStatus::Waiting,
                "Waiting for schedule",
                "Stay Awake: waiting for schedule (No upcoming window)",
                runtime::TrayIcon::Waiting,
            ),
            (
                runtime::TrayStatus::Paused,
                "Paused for this window",
                "Stay Awake: paused for this window (No upcoming window)",
                runtime::TrayIcon::Inactive,
            ),
            (
                runtime::TrayStatus::ActiveOverride,
                "Active (manual override)",
                "Stay Awake: active by manual override (No upcoming window)",
                runtime::TrayIcon::Active,
            ),
        ];
        let mut sink = RecordingSink::default();
        for (status, menu_text, tooltip, icon) in cases {
            let state = snapshot(status);
            let presentation = build_tray_presentation(PresentationInput {
                snapshot: &state,
                bundle: &bundle,
                locale: "en",
                clock: &clock,
            });
            assert_eq!(presentation.status, status);
            assert_eq!(presentation.menu_text, menu_text);
            assert_eq!(presentation.tooltip, tooltip);
            assert_eq!(presentation.icon, icon);
            refresh_tray_status(
                TrayRefreshInput {
                    route: TrayRoute::Tick,
                    presentation: &presentation,
                },
                &mut sink,
            )
            .expect("recording sink accepts publication");
        }
        assert_eq!(sink.calls.len(), cases.len());
    }

    #[test]
    fn refresh_publishes_exactly_one_named_presentation_per_call() {
        let presentation = TrayPresentation {
            status: runtime::TrayStatus::Waiting,
            menu_text: "Waiting for schedule".to_owned(),
            tooltip: "Stay Awake: waiting for schedule (No upcoming window)".to_owned(),
            icon: runtime::TrayIcon::Waiting,
        };
        let routes = [
            TrayRoute::Startup,
            TrayRoute::Tick,
            TrayRoute::LanguageChanged,
        ];
        let mut sink = RecordingSink::default();
        for route in routes {
            refresh_tray_status(
                TrayRefreshInput {
                    route,
                    presentation: &presentation,
                },
                &mut sink,
            )
            .expect("recording sink accepts publication");
        }
        assert_eq!(sink.calls.len(), routes.len());
        assert_eq!(
            sink.calls,
            routes
                .into_iter()
                .map(|route| (route, presentation.clone()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn runtime_routes_publish_once_and_language_refreshes_an_initialized_snapshot() {
        let clock = FixedClock(at(10));
        let bundle = i18n::create_bundle("en");
        let config = AppConfig::default();
        let routes = [
            (TrayRoute::Startup, 0, 0, runtime::TrayStatus::Inactive),
            (TrayRoute::TrayToggle, 1, 0, runtime::TrayStatus::Active),
            (TrayRoute::Hotkey, 1, 0, runtime::TrayStatus::Active),
            (TrayRoute::IpcToggle, 1, 0, runtime::TrayStatus::Active),
            (TrayRoute::Tick, 0, 0, runtime::TrayStatus::Inactive),
            (
                TrayRoute::WifiChanged { active: true },
                0,
                0,
                runtime::TrayStatus::Inactive,
            ),
            (
                TrayRoute::ConfigChanged {
                    policy_changed: false,
                },
                0,
                0,
                runtime::TrayStatus::Inactive,
            ),
            (
                TrayRoute::ProfileLoaded {
                    policy_changed: true,
                },
                0,
                0,
                runtime::TrayStatus::Inactive,
            ),
            (TrayRoute::Resume, 0, 0, runtime::TrayStatus::Inactive),
            (TrayRoute::Quit, 0, 0, runtime::TrayStatus::Inactive),
        ];
        for (route, starts, stops, status) in routes {
            let mut supervisor = runtime::Supervisor::new();
            let mut backend = RecordingBackend::default();
            let mut sink = RecordingSink::default();
            let result = handle_runtime_route(
                RuntimeRouteInput {
                    route,
                    config: &config,
                    clock: &clock,
                    bundle: &bundle,
                },
                &mut supervisor,
                &mut backend,
                &mut sink,
            );
            assert!(result.is_ok(), "{route:?}");
            assert_eq!(sink.calls.len(), 1, "{route:?}");
            assert_eq!(backend.start_calls, starts, "{route:?}");
            assert_eq!(backend.stop_calls, stops, "{route:?}");
            assert_eq!(sink.calls[0].1.status, status, "{route:?}");
        }

        let mut supervisor = runtime::Supervisor::new();
        let mut backend = RecordingBackend::default();
        let mut sink = RecordingSink::default();
        let startup = handle_runtime_route(
            RuntimeRouteInput {
                route: TrayRoute::Startup,
                config: &config,
                clock: &clock,
                bundle: &bundle,
            },
            &mut supervisor,
            &mut backend,
            &mut sink,
        );
        assert!(startup.is_ok());
        let language = handle_runtime_route(
            RuntimeRouteInput {
                route: TrayRoute::LanguageChanged,
                config: &config,
                clock: &clock,
                bundle: &bundle,
            },
            &mut supervisor,
            &mut backend,
            &mut sink,
        );
        assert!(language.is_ok());
        assert_eq!(backend.start_calls, 0);
        assert_eq!(backend.stop_calls, 0);
        assert_eq!(sink.calls.len(), 2);
    }

    #[test]
    fn language_change_without_a_snapshot_returns_an_error_without_backend_or_publication() {
        let clock = FixedClock(at(10));
        let bundle = i18n::create_bundle("en");
        let config = AppConfig::default();
        let mut supervisor = runtime::Supervisor::new();
        let mut backend = RecordingBackend::default();
        let mut sink = RecordingSink::default();
        let result = handle_runtime_route(
            RuntimeRouteInput {
                route: TrayRoute::LanguageChanged,
                config: &config,
                clock: &clock,
                bundle: &bundle,
            },
            &mut supervisor,
            &mut backend,
            &mut sink,
        );
        assert!(result.is_err());
        assert_eq!(backend.start_calls, 0);
        assert_eq!(backend.stop_calls, 0);
        assert!(sink.calls.is_empty());
    }

    #[test]
    fn wifi_routes_publish_only_active_or_inactive_states() {
        let clock = FixedClock(at(10));
        let bundle = i18n::create_bundle("en");
        let config = AppConfig {
            mode: AppMode::WiFi,
            schedule_enabled: true,
            ..AppConfig::default()
        };
        for (route, initially_active, expected) in [
            (
                TrayRoute::WifiChanged { active: true },
                false,
                runtime::TrayStatus::Active,
            ),
            (
                TrayRoute::WifiChanged { active: false },
                true,
                runtime::TrayStatus::Inactive,
            ),
        ] {
            let mut supervisor = runtime::Supervisor::new();
            let mut backend = RecordingBackend {
                active: initially_active,
                ..RecordingBackend::default()
            };
            let mut sink = RecordingSink::default();
            let _ = handle_runtime_route(
                RuntimeRouteInput {
                    route,
                    config: &config,
                    clock: &clock,
                    bundle: &bundle,
                },
                &mut supervisor,
                &mut backend,
                &mut sink,
            );
            assert_eq!(sink.calls.len(), 1);
            assert_eq!(sink.calls[0].1.status, expected);
        }
    }

    #[test]
    fn language_change_builds_a_fresh_presentation_from_the_replacement_bundle() {
        let clock = FixedClock(at(10));
        let state = snapshot(runtime::TrayStatus::Active);
        let english = build_tray_presentation(PresentationInput {
            snapshot: &state,
            bundle: &i18n::create_bundle("en"),
            locale: "en",
            clock: &clock,
        });
        let spanish = build_tray_presentation(PresentationInput {
            snapshot: &state,
            bundle: &i18n::create_bundle("es"),
            locale: "es",
            clock: &clock,
        });
        assert_eq!(english.menu_text, "Active");
        assert_eq!(spanish.menu_text, "Activo");
        assert_ne!(english.menu_text, spanish.menu_text);
        let mut sink = RecordingSink::default();
        refresh_tray_status(
            TrayRefreshInput {
                route: TrayRoute::LanguageChanged,
                presentation: &spanish,
            },
            &mut sink,
        )
        .expect("recording sink accepts publication");
        assert_eq!(sink.calls, vec![(TrayRoute::LanguageChanged, spanish)]);
    }

    #[test]
    fn failed_backend_operations_publish_the_actual_inactive_state() {
        let clock = FixedClock(at(10));
        let bundle = i18n::create_bundle("en");
        let config = AppConfig {
            mode: AppMode::Manual,
            schedule_enabled: true,
            schedule_days: vec!["mon".to_owned()],
            ..AppConfig::default()
        };
        let mut supervisor = runtime::Supervisor::new();
        let mut backend = RecordingBackend {
            fail_start: true,
            ..RecordingBackend::default()
        };
        let mut sink = RecordingSink::default();
        assert!(handle_runtime_route(
            RuntimeRouteInput {
                route: TrayRoute::Startup,
                config: &config,
                clock: &clock,
                bundle: &bundle,
            },
            &mut supervisor,
            &mut backend,
            &mut sink,
        )
        .is_err());
        assert_eq!(sink.calls.len(), 1);
        assert_eq!(sink.calls[0].1.status, runtime::TrayStatus::Inactive);
    }

    #[test]
    fn checkbox_startup_matrix_uses_the_real_supervisor_and_truthful_publication() {
        let bundle = i18n::create_bundle("en");
        let cases = [
            (AppMode::Manual, false, 10, 0, runtime::TrayStatus::Inactive),
            (AppMode::Manual, false, 18, 0, runtime::TrayStatus::Inactive),
            (AppMode::Manual, true, 10, 1, runtime::TrayStatus::Active),
            (AppMode::Manual, true, 18, 0, runtime::TrayStatus::Waiting),
            (AppMode::AlwaysOn, false, 10, 1, runtime::TrayStatus::Active),
            (AppMode::AlwaysOn, false, 18, 1, runtime::TrayStatus::Active),
            (AppMode::AlwaysOn, true, 10, 1, runtime::TrayStatus::Active),
            (AppMode::AlwaysOn, true, 18, 0, runtime::TrayStatus::Waiting),
            (
                AppMode::Scheduled,
                false,
                10,
                0,
                runtime::TrayStatus::Inactive,
            ),
            (AppMode::Scheduled, true, 10, 1, runtime::TrayStatus::Active),
            (
                AppMode::Scheduled,
                true,
                18,
                0,
                runtime::TrayStatus::Waiting,
            ),
        ];

        for (mode, schedule_enabled, hour, expected_starts, expected_status) in cases {
            let config = AppConfig {
                mode,
                schedule_enabled,
                schedule_days: vec!["mon".to_owned()],
                ..AppConfig::default()
            };
            let clock = FixedClock(at(hour));
            let mut supervisor = runtime::Supervisor::new();
            let mut backend = RecordingBackend::default();
            let mut sink = RecordingSink::default();

            let snapshot = handle_runtime_route(
                RuntimeRouteInput {
                    route: TrayRoute::Startup,
                    config: &config,
                    clock: &clock,
                    bundle: &bundle,
                },
                &mut supervisor,
                &mut backend,
                &mut sink,
            )
            .expect("startup reconciliation succeeds");

            assert_eq!(
                backend.start_calls, expected_starts,
                "{config:?} at {hour}:00"
            );
            assert_eq!(backend.stop_calls, 0, "{config:?} at {hour}:00");
            assert_eq!(snapshot.status, expected_status, "{config:?} at {hour}:00");
            assert_eq!(sink.calls.len(), 1, "{config:?} at {hour}:00");
            assert_eq!(
                sink.calls[0].1.status, expected_status,
                "{config:?} at {hour}:00"
            );
        }
    }
}

/// Check whether accessibility permission is granted (macOS-specific).
#[tauri::command]
fn check_accessibility(
    checker: tauri::State<'_, Arc<dyn PermissionChecker>>,
) -> Result<bool, String> {
    Ok(checker.check_accessibility())
}

/// Open the OS accessibility settings so the user can grant permission.
#[tauri::command]
fn request_accessibility(
    checker: tauri::State<'_, Arc<dyn PermissionChecker>>,
) -> Result<(), String> {
    checker.request_accessibility()
}

/// Query the OS autostart manager for the current state.
#[tauri::command]
fn get_autostart_enabled(app: tauri::AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

/// Enable or disable launch-at-login via the OS autostart manager.
#[tauri::command]
fn set_autostart_enabled(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    if enabled {
        manager.enable().map_err(|e| e.to_string())
    } else {
        manager.disable().map_err(|e| e.to_string())
    }
}

/// Hide the settings window (called from the settings webview on Save/Cancel).
#[tauri::command]
fn close_settings_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("settings") {
        win.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Return a translated string for the given message key.
#[tauri::command]
fn get_translation(
    key: String,
    bundle: tauri::State<'_, Arc<Mutex<I18nBundle>>>,
) -> Result<String, String> {
    let b = bundle
        .lock()
        .map_err(|e| format!("I18n lock poisoned: {}", e))?;
    Ok(i18n::t(&b, &key))
}

/// Return all saved profiles.
#[tauri::command]
fn list_profiles(config: tauri::State<'_, Arc<Mutex<AppConfig>>>) -> Result<Vec<Profile>, String> {
    let cfg = config
        .lock()
        .map_err(|e| format!("Config lock poisoned: {}", e))?;
    Ok(cfg.profiles.clone())
}

/// Save the current settings as a named profile.
#[cfg(desktop)]
#[tauri::command]
async fn save_profile(
    name: String,
    runtime: tauri::State<'_, owner::RuntimeHandle>,
) -> Result<(), String> {
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        use owner::ConfigOrigin;

        let edit = move |config: &mut AppConfig| {
            let name = name.trim().to_string();
            if name.is_empty() || name.len() > 64 {
                return Err("Profile name must be 1–64 characters".to_owned());
            }
            if config.profiles.len() >= 50 && !config.profiles.iter().any(|p| p.name == name) {
                return Err("Maximum of 50 profiles reached".to_owned());
            }
            let profile = Profile {
                name: name.clone(),
                jiggle_mode: config.jiggle_mode.clone(),
                interval_secs: config.interval_secs,
                schedule_enabled: config.schedule_enabled,
                schedule_start_hour: config.schedule_start_hour,
                schedule_start_minute: config.schedule_start_minute,
                schedule_end_hour: config.schedule_end_hour,
                schedule_end_minute: config.schedule_end_minute,
                schedule_days: config.schedule_days.clone(),
            };
            if let Some(existing) = config.profiles.iter_mut().find(|p| p.name == name) {
                *existing = profile;
            } else {
                config.profiles.push(profile);
            }
            config.active_profile = name;
            Ok(())
        };
        let pending = runtime.update_config(ConfigOrigin::Other, edit, |_, _| {})?;
        pending
            .wait(owner::REQUEST_TIMEOUT)?
            .error
            .map_or(Ok(()), Err)
    })
    .await
    .map_err(|error| format!("profile save task failed: {error}"))?
}

#[cfg(desktop)]
#[tauri::command]
async fn toggle_engine(runtime: tauri::State<'_, owner::RuntimeHandle>) -> Result<String, String> {
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let reply = runtime.request(TrayRoute::IpcToggle)?;
        let reply = reply.wait(owner::REQUEST_TIMEOUT)?;
        if let Some(error) = reply.error {
            return Err(error);
        }
        state_name(reply.snapshot)
    })
    .await
    .map_err(|error| format!("toggle task failed: {error}"))?
}

#[cfg(desktop)]
#[tauri::command]
async fn get_engine_state(
    runtime: tauri::State<'_, owner::RuntimeHandle>,
) -> Result<String, String> {
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let reply = runtime.query()?.wait(owner::REQUEST_TIMEOUT)?;
        state_name(reply.snapshot)
    })
    .await
    .map_err(|error| format!("state query task failed: {error}"))?
}

#[cfg(desktop)]
#[tauri::command]
async fn save_config(
    app: tauri::AppHandle,
    new_config: AppConfig,
    runtime: tauri::State<'_, owner::RuntimeHandle>,
    monitor: tauri::State<'_, Arc<Mutex<wifi::WifiMonitor>>>,
) -> Result<(), String> {
    let runtime = runtime.inner().clone();
    let monitor = monitor.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        use owner::ConfigOrigin;

        let edit = move |config: &mut AppConfig| {
            *config = new_config;
            Ok(())
        };
        let effects = move |previous: &AppConfig, candidate: &AppConfig| {
            apply_config_effects(&app, &monitor, previous, candidate);
        };
        let pending = runtime.update_config(ConfigOrigin::Settings, edit, effects)?;
        pending
            .wait(owner::REQUEST_TIMEOUT)?
            .error
            .map_or(Ok(()), Err)
    })
    .await
    .map_err(|error| format!("config save task failed: {error}"))?
}

#[cfg(desktop)]
fn apply_config_effects(
    app: &tauri::AppHandle,
    monitor: &Arc<Mutex<wifi::WifiMonitor>>,
    previous: &AppConfig,
    candidate: &AppConfig,
) {
    use tauri_plugin_autostart::ManagerExt;

    let manager = app.autolaunch();
    let autostart_result = if candidate.autostart {
        manager.enable()
    } else {
        manager.disable()
    };
    if let Err(error) = autostart_result {
        log::warn!("Failed to sync autostart: {error}");
    }

    let wifi_changed = previous.wifi_enabled() != candidate.wifi_enabled()
        || (candidate.wifi_enabled() && previous.wifi.networks != candidate.wifi.networks);
    if !wifi_changed {
        return;
    }
    match monitor.lock() {
        Ok(mut monitor) if candidate.wifi_enabled() => {
            if let Err(error) = monitor.restart() {
                log::warn!("WiFi monitor restart failed: {error}");
            }
        }
        Ok(mut monitor) => {
            if let Err(error) = monitor.stop() {
                log::warn!("WiFi monitor stop failed: {error}");
            }
        }
        Err(_) => log::error!("WiFi monitor lock poisoned"),
    }
}

#[cfg(desktop)]
#[tauri::command]
async fn load_profile(
    name: String,
    runtime: tauri::State<'_, owner::RuntimeHandle>,
) -> Result<(), String> {
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        use owner::ConfigOrigin;

        let edit = move |config: &mut AppConfig| {
            let profile = config
                .profiles
                .iter()
                .find(|profile| profile.name == name)
                .cloned()
                .ok_or_else(|| format!("Profile '{name}' not found"))?;
            config.jiggle_mode = profile.jiggle_mode;
            config.interval_secs = profile.interval_secs;
            config.schedule_enabled = profile.schedule_enabled;
            config.schedule_start_hour = profile.schedule_start_hour;
            config.schedule_start_minute = profile.schedule_start_minute;
            config.schedule_end_hour = profile.schedule_end_hour;
            config.schedule_end_minute = profile.schedule_end_minute;
            config.schedule_days = profile.schedule_days;
            config.active_profile = name;
            Ok(())
        };
        let pending = runtime.update_config(ConfigOrigin::Profile, edit, |_, _| {})?;
        pending
            .wait(owner::REQUEST_TIMEOUT)?
            .error
            .map_or(Ok(()), Err)
    })
    .await
    .map_err(|error| format!("profile load task failed: {error}"))?
}

#[cfg(desktop)]
#[tauri::command]
async fn set_language(
    language: String,
    runtime: tauri::State<'_, owner::RuntimeHandle>,
) -> Result<(), String> {
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        use owner::ConfigOrigin;

        let edit = move |config: &mut AppConfig| {
            config.language = language;
            Ok(())
        };
        let pending = runtime.update_config(ConfigOrigin::Language, edit, |_, _| {})?;
        pending
            .wait(owner::REQUEST_TIMEOUT)?
            .error
            .map_or(Ok(()), Err)
    })
    .await
    .map_err(|error| format!("language change task failed: {error}"))?
}

#[cfg(desktop)]
fn state_name(snapshot: Option<runtime::RuntimeSnapshot>) -> Result<String, String> {
    let snapshot = snapshot.ok_or_else(|| "runtime has no snapshot".to_owned())?;
    Ok(if snapshot.engine_active {
        "active"
    } else {
        "idle"
    }
    .to_owned())
}

/// Delete a named profile.
#[cfg(desktop)]
#[tauri::command]
async fn delete_profile(
    name: String,
    runtime: tauri::State<'_, owner::RuntimeHandle>,
) -> Result<(), String> {
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        use owner::ConfigOrigin;

        let edit = move |config: &mut AppConfig| {
            config.profiles.retain(|profile| profile.name != name);
            if config.active_profile == name {
                config.active_profile = "Default".to_owned();
            }
            Ok(())
        };
        let pending = runtime.update_config(ConfigOrigin::Other, edit, |_, _| {})?;
        pending
            .wait(owner::REQUEST_TIMEOUT)?
            .error
            .map_or(Ok(()), Err)
    })
    .await
    .map_err(|error| format!("profile deletion task failed: {error}"))?
}

/// Update the global hotkey at runtime: unregister the old shortcut, register
/// the new one, and persist the change.
#[cfg(desktop)]
#[tauri::command]
async fn update_global_hotkey(
    app: tauri::AppHandle,
    hotkey: String,
    runtime: tauri::State<'_, owner::RuntimeHandle>,
) -> Result<(), String> {
    let runtime = runtime.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        use owner::ConfigOrigin;
        use tauri_plugin_global_shortcut::GlobalShortcutExt;

        let new_shortcut =
            parse_shortcut_string(&hotkey).ok_or_else(|| format!("Invalid shortcut: {hotkey}"))?;
        let shortcuts = app.global_shortcut();
        let _ = shortcuts.unregister_all();
        shortcuts
            .register(new_shortcut)
            .map_err(|error| format!("Failed to register shortcut: {error}"))?;
        let edit = move |config: &mut AppConfig| {
            config.global_hotkey = hotkey;
            Ok(())
        };
        let pending = runtime.update_config(ConfigOrigin::Other, edit, |_, _| {})?;
        pending
            .wait(owner::REQUEST_TIMEOUT)?
            .error
            .map_or(Ok(()), Err)
    })
    .await
    .map_err(|error| format!("hotkey update task failed: {error}"))?
}

#[tauri::command]
fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Return the compile-time target OS (`"linux"`, `"macos"`, `"windows"`) so the frontend can
/// branch on platform without relying on `navigator.platform` UA-sniffing, which is
/// deprecated and does not reliably distinguish Linux from Windows across webview engines.
/// Mirrors the `get_networkmanager_status` precedent of surfacing platform facts via a
/// backend-sourced command rather than frontend detection.
#[tauri::command]
fn get_platform_os() -> String {
    std::env::consts::OS.to_string()
}

/// Return the SSID of the currently connected WiFi network, or `None` if
/// disconnected. Delegates directly to the platform detection function.
#[tauri::command]
fn get_current_wifi() -> Result<Option<String>, String> {
    Ok(wifi::detect_current_ssid())
}

/// Check Location Services authorization status for WiFi SSID reading.
#[tauri::command]
fn get_location_status() -> Result<String, String> {
    let status = wifi::get_location_status();
    match status {
        wifi::LocationStatus::NotDetermined => Ok("not_determined".to_string()),
        wifi::LocationStatus::Restricted => Ok("restricted".to_string()),
        wifi::LocationStatus::Denied => Ok("denied".to_string()),
        wifi::LocationStatus::Authorized => Ok("authorized".to_string()),
    }
}

/// Request Location Services permission for WiFi SSID detection.
#[tauri::command]
fn request_location_permission() -> Result<(), String> {
    wifi::request_location();
    Ok(())
}

/// Check NetworkManager availability for reactive WiFi monitoring on Linux.
/// Returns `"available"`, `"unavailable"`, or `"not_applicable"` (non-Linux platforms) — a
/// single consistent shape the frontend can branch on without needing its own platform
/// detection (there is no `tauri-plugin-os` dependency today).
#[tauri::command]
fn get_networkmanager_status() -> Result<String, String> {
    let status = wifi::get_networkmanager_status();
    match status {
        wifi::NetworkManagerStatus::Available => Ok("available".to_string()),
        wifi::NetworkManagerStatus::Unavailable => Ok("unavailable".to_string()),
        wifi::NetworkManagerStatus::NotApplicable => Ok("not_applicable".to_string()),
    }
}

/// Parse a human-readable shortcut string like `"CmdOrCtrl+Shift+J"` into a
/// [`Shortcut`] that can be registered with the global-shortcut plugin.
#[cfg(desktop)]
fn parse_shortcut_string(s: &str) -> Option<tauri_plugin_global_shortcut::Shortcut> {
    use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};

    let parts: Vec<&str> = s.split('+').map(|p| p.trim()).collect();
    if parts.is_empty() {
        return None;
    }

    let mut mods: Option<Modifiers> = None;
    let mut add_mod = |m: Modifiers| {
        mods = Some(match mods {
            Some(existing) => existing | m,
            None => m,
        });
    };

    for part in &parts[..parts.len().saturating_sub(1)] {
        match part.to_lowercase().as_str() {
            "ctrl" | "control" => add_mod(Modifiers::CONTROL),
            "cmd" | "meta" | "super" | "command" => add_mod(Modifiers::META),
            "cmdorctrl" | "commandorcontrol" => {
                #[cfg(target_os = "macos")]
                add_mod(Modifiers::META);
                #[cfg(not(target_os = "macos"))]
                add_mod(Modifiers::CONTROL);
            }
            "shift" => add_mod(Modifiers::SHIFT),
            "alt" | "option" => add_mod(Modifiers::ALT),
            _ => return None,
        }
    }

    let key_str = parts.last()?.to_uppercase();
    let code = match key_str.as_str() {
        "A" => Code::KeyA,
        "B" => Code::KeyB,
        "C" => Code::KeyC,
        "D" => Code::KeyD,
        "E" => Code::KeyE,
        "F" => Code::KeyF,
        "G" => Code::KeyG,
        "H" => Code::KeyH,
        "I" => Code::KeyI,
        "J" => Code::KeyJ,
        "K" => Code::KeyK,
        "L" => Code::KeyL,
        "M" => Code::KeyM,
        "N" => Code::KeyN,
        "O" => Code::KeyO,
        "P" => Code::KeyP,
        "Q" => Code::KeyQ,
        "R" => Code::KeyR,
        "S" => Code::KeyS,
        "T" => Code::KeyT,
        "U" => Code::KeyU,
        "V" => Code::KeyV,
        "W" => Code::KeyW,
        "X" => Code::KeyX,
        "Y" => Code::KeyY,
        "Z" => Code::KeyZ,
        "0" => Code::Digit0,
        "1" => Code::Digit1,
        "2" => Code::Digit2,
        "3" => Code::Digit3,
        "4" => Code::Digit4,
        "5" => Code::Digit5,
        "6" => Code::Digit6,
        "7" => Code::Digit7,
        "8" => Code::Digit8,
        "9" => Code::Digit9,
        "F1" => Code::F1,
        "F2" => Code::F2,
        "F3" => Code::F3,
        "F4" => Code::F4,
        "F5" => Code::F5,
        "F6" => Code::F6,
        "F7" => Code::F7,
        "F8" => Code::F8,
        "F9" => Code::F9,
        "F10" => Code::F10,
        "F11" => Code::F11,
        "F12" => Code::F12,
        "SPACE" => Code::Space,
        "ENTER" | "RETURN" => Code::Enter,
        "ESCAPE" | "ESC" => Code::Escape,
        "TAB" => Code::Tab,
        _ => return None,
    };

    Some(Shortcut::new(mods, code))
}

/// Show the settings window, creating it dynamically if the config-defined
/// instance was closed by the user.
#[cfg(desktop)]
fn show_settings_window(app: &tauri::AppHandle) {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    #[cfg(target_os = "macos")]
    set_macos_regular_app();

    if let Some(win) = app.get_webview_window("settings") {
        let _ = win.show();
        let _ = win.set_focus();
    } else if let Err(e) =
        WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
            .title("Stay Awake Settings")
            .inner_size(480.0, 720.0)
            .resizable(false)
            .center()
            .build()
    {
        log::error!("Failed to create settings window: {}", e);
    }
}

/// Show the About window with version and author information.
#[cfg(desktop)]
fn show_about_window(app: &tauri::AppHandle) {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    #[cfg(target_os = "macos")]
    set_macos_regular_app();

    if let Some(win) = app.get_webview_window("about") {
        let _ = win.show();
        let _ = win.set_focus();
    } else if let Err(e) =
        WebviewWindowBuilder::new(app, "about", WebviewUrl::App("about.html".into()))
            .title("About Stay Awake")
            .inner_size(360.0, 340.0)
            .resizable(false)
            .center()
            .build()
    {
        log::error!("Failed to create about window: {}", e);
    }
}

/// Configure the macOS app as an Accessory (no Dock icon, no menu bar).
/// Must be called before the Tauri builder runs.
#[cfg(target_os = "macos")]
fn configure_macos_app() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSMenu};

    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        let empty_menu = NSMenu::new(mtm);
        app.setMainMenu(Some(&empty_menu));
    }
}

/// Temporarily promote to Regular app (shows in Dock + Cmd-Tab with proper icon).
#[cfg(target_os = "macos")]
fn set_macos_regular_app() {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSImage};
    use objc2_foundation::NSData;

    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

        // Set the app icon programmatically (works even in dev mode without a bundle).
        let icon_bytes = include_bytes!("../icons/icon.png");
        let data =
            unsafe { NSData::dataWithBytes_length(icon_bytes.as_ptr().cast(), icon_bytes.len()) };
        if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
            unsafe { app.setApplicationIconImage(Some(&image)) };
        }
    }
}

/// Demote back to Accessory (hides from Dock + Cmd-Tab).
#[cfg(target_os = "macos")]
fn set_macos_accessory_app() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    }
}

/// Create a dimmed version of an RGBA icon by reducing opacity.
fn create_dimmed_icon(icon: &tauri::image::Image<'_>) -> tauri::image::Image<'static> {
    let rgba = icon.rgba().to_vec();
    let mut dimmed = rgba.clone();
    // Reduce alpha channel to ~40% for a "greyed out" look
    for i in (3..dimmed.len()).step_by(4) {
        dimmed[i] = (dimmed[i] as f32 * 0.4) as u8;
    }
    tauri::image::Image::new_owned(dimmed, icon.width(), icon.height())
}

#[cfg(test)]
fn is_within_schedule(cfg: &AppConfig) -> bool {
    cfg.schedule_enabled && engine::is_within_schedule(cfg)
}

/// Pure predicate: does this panic message describe the appindicator dynamic-library
/// load failure raised by `libappindicator-sys`? Matched on message text only — no
/// coupling to the library's private soname-search logic.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn is_appindicator_panic_message(message: &str) -> bool {
    message.contains("appindicator")
}

/// Pure selection logic: which Fluent key names the tray accessibility-warning item's
/// text when the checked condition is *not* satisfied?
///
/// Linux never had a macOS-style accessibility permission to grant — the real gating
/// condition there is whether `xdotool` is present on `PATH` (see
/// `LinuxPermissionChecker::check_accessibility`). Kept as a pure `bool -> &str` function,
/// separate from any `#[cfg(target_os = ...)]` gate, so the decision is unit-testable on
/// every build target.
fn warn_key_for(is_linux: bool) -> &'static str {
    if is_linux {
        "tray-xdotool-missing"
    } else {
        "tray-grant-accessibility"
    }
}

/// Pure selection logic: which Fluent key names the tray accessibility item's text when
/// the checked condition *is* satisfied? Counterpart to `warn_key_for`.
fn ok_key_for(is_linux: bool) -> &'static str {
    if is_linux {
        "tray-xdotool-ok"
    } else {
        "tray-accessibility-ok"
    }
}

/// Fluent key for the tray accessibility-warning item on this build target.
#[cfg(target_os = "linux")]
fn accessibility_warn_key() -> &'static str {
    warn_key_for(true)
}

/// Fluent key for the tray accessibility-warning item on this build target.
#[cfg(not(target_os = "linux"))]
fn accessibility_warn_key() -> &'static str {
    warn_key_for(false)
}

/// Fluent key for the tray accessibility-OK item on this build target.
#[cfg(target_os = "linux")]
fn accessibility_ok_key() -> &'static str {
    ok_key_for(true)
}

/// Fluent key for the tray accessibility-OK item on this build target.
#[cfg(not(target_os = "linux"))]
fn accessibility_ok_key() -> &'static str {
    ok_key_for(false)
}

/// Build and run the Tauri application (called from `main.rs`).
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize logging as early as possible.
    let _ = env_logger::try_init();

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_config,
            #[cfg(desktop)]
            save_config,
            #[cfg(desktop)]
            get_engine_state,
            #[cfg(desktop)]
            toggle_engine,
            check_accessibility,
            request_accessibility,
            get_autostart_enabled,
            set_autostart_enabled,
            close_settings_window,
            get_translation,
            #[cfg(desktop)]
            set_language,
            list_profiles,
            #[cfg(desktop)]
            save_profile,
            #[cfg(desktop)]
            load_profile,
            #[cfg(desktop)]
            delete_profile,
            #[cfg(desktop)]
            update_global_hotkey,
            get_app_version,
            get_platform_os,
            get_current_wifi,
            get_location_status,
            request_location_permission,
            get_networkmanager_status,
        ])
        .plugin(tauri_plugin_single_instance::init(|_app, _args, _cwd| {
            log::info!("Another instance attempted to start -- focusing existing instance");
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let loaded_config = AppConfig::load().unwrap_or_else(|_| {
                log::info!("No config found; using defaults");
                let cfg = AppConfig::default();
                let _ = cfg.save();
                cfg
            });
            let shared_config: Arc<Mutex<AppConfig>> = Arc::new(Mutex::new(loaded_config.clone()));

            let bundle = i18n::create_bundle(&loaded_config.language);

            let clock: Arc<dyn schedule::Clock> = Arc::new(schedule::SystemClock);
            let initial_snapshot = initial_runtime_snapshot(InitialSnapshotInput {
                config: &loaded_config,
                clock: clock.as_ref(),
            })?;
            let initial_presentation = build_tray_presentation(PresentationInput {
                snapshot: &initial_snapshot,
                bundle: &bundle,
                locale: &loaded_config.language,
                clock: clock.as_ref(),
            });
            let shared_bundle: Arc<Mutex<I18nBundle>> = Arc::new(Mutex::new(bundle));
            let perm_checker: Arc<dyn PermissionChecker> =
                Arc::from(platform::create_permission_checker());

            let has_accessibility = perm_checker.check_accessibility();
            let needs_mouse = loaded_config.jiggle_mode != JiggleMode::PowerOnly;
            if needs_mouse && !has_accessibility {
                log::warn!(
                    "Accessibility permission not granted — \
                     mouse jiggle modes will not work until access is granted"
                );
            }

            // Managed mirrors are read by existing compatibility commands.
            app.manage(Arc::clone(&shared_config));
            app.manage(Arc::clone(&perm_checker));
            app.manage(Arc::clone(&shared_bundle));

            let wifi_monitor =
                wifi::WifiMonitor::new(Arc::clone(&shared_config), app.handle().clone());
            let shared_wifi_monitor: Arc<Mutex<wifi::WifiMonitor>> =
                Arc::new(Mutex::new(wifi_monitor));

            app.manage(Arc::clone(&shared_wifi_monitor));

            // Local mirror of WifiStatePayload used only for deserialization.
            #[derive(serde::Deserialize)]
            struct WifiStateEvent {
                ssid: Option<String>,
                active: bool,
            }

            {
                use tauri::Listener;
                let app_handle = app.handle().clone();
                app.listen("wifi-state-changed", move |event| {
                    if let Ok(payload) = serde_json::from_str::<WifiStateEvent>(event.payload()) {
                        let _ = payload.active;
                        if let Some(runtime) = app_handle.try_state::<owner::RuntimeHandle>() {
                            if let Err(error) = runtime.post_wifi(owner::WifiObservation { ssid: payload.ssid }) {
                                log::error!("WiFi update could not reach runtime: {error}");
                            }
                        }
                    }
                });
            }

            #[cfg(desktop)]
            {
                use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
                use tauri::tray::TrayIconBuilder;

                // Resolve tray labels from the i18n bundle.
                let tray_bundle = i18n::create_bundle(&loaded_config.language);
                let tr = |id: &str| i18n::t(&tray_bundle, id);

                let toggle_item =
                    MenuItem::with_id(
                        app,
                        "toggle_active",
                        &initial_presentation.menu_text,
                        true,
                        None::<&str>,
                    )?;

                let mode_power = MenuItem::with_id(
                    app,
                    "mode_power",
                    tr("tray-mode-power"),
                    true,
                    None::<&str>,
                )?;
                let mode_subtle = MenuItem::with_id(
                    app,
                    "mode_subtle",
                    tr("tray-mode-subtle"),
                    true,
                    None::<&str>,
                )?;
                let mode_zen =
                    MenuItem::with_id(app, "mode_zen", tr("tray-mode-zen"), true, None::<&str>)?;

                let mode_circle = MenuItem::with_id(
                    app,
                    "mode_circle",
                    tr("tray-mode-circle"),
                    true,
                    None::<&str>,
                )?;

                let mode_submenu = Submenu::with_id_and_items(
                    app,
                    "mode",
                    tr("tray-mode"),
                    true,
                    &[&mode_power, &mode_subtle, &mode_zen, &mode_circle],
                )?;

                // Accessibility permission indicator.
                let (acc_text, acc_enabled) = if has_accessibility {
                    (
                        format!("\u{1f7e2} {}", tr(accessibility_ok_key())),
                        false,
                    )
                } else {
                    (
                        format!("\u{26a0}\u{fe0f} {}", tr(accessibility_warn_key())),
                        true,
                    )
                };
                let accessibility_item = MenuItem::with_id(
                    app,
                    "grant_accessibility",
                    acc_text,
                    acc_enabled,
                    None::<&str>,
                )?;

                let sep1 = PredefinedMenuItem::separator(app)?;
                let sep2 = PredefinedMenuItem::separator(app)?;
                let sep3 = PredefinedMenuItem::separator(app)?;
                let sep4 = PredefinedMenuItem::separator(app)?;
                let settings_item =
                    MenuItem::with_id(app, "settings", tr("tray-settings"), true, None::<&str>)?;
                let about_item =
                    MenuItem::with_id(app, "about", tr("tray-about"), true, None::<&str>)?;
                let quit_item =
                    MenuItem::with_id(app, "quit", tr("tray-quit"), true, None::<&str>)?;

                let menu = Menu::with_items(
                    app,
                    &[
                        &toggle_item,
                        &sep1,
                        &mode_submenu,
                        &sep2,
                        &accessibility_item,
                        &sep3,
                        &settings_item,
                        &about_item,
                        &sep4,
                        &quit_item,
                    ],
                )?;

                let acc_ok_text = format!("\u{1f7e2} {}", tr(accessibility_ok_key()));

                let perm_for_tray = Arc::clone(&perm_checker);
                let accessibility_clone = accessibility_item.clone();

                let tray_icon_bytes = include_bytes!("../icons/tray-icon@2x.png");
                let tray_img = tauri::image::Image::from_bytes(tray_icon_bytes).expect("tray icon");
                let active_icon = tauri::image::Image::new_owned(
                    tray_img.rgba().to_vec(),
                    tray_img.width(),
                    tray_img.height(),
                );
                let inactive_icon = create_dimmed_icon(&tray_img);

                // Under `panic = "abort"` (release profile), a missing
                // libayatana-appindicator3 causes an unavoidable abort inside
                // TrayIconBuilder::build(). This does NOT prevent the abort — it only
                // makes the failure actionable by naming the missing package on stderr
                // before the process goes down.
                #[cfg(target_os = "linux")]
                {
                    let previous_hook = std::panic::take_hook();
                    std::panic::set_hook(Box::new(move |info| {
                        previous_hook(info);
                        let message = info
                            .payload()
                            .downcast_ref::<String>()
                            .map(String::as_str)
                            .or_else(|| info.payload().downcast_ref::<&str>().copied());
                        if let Some(message) = message {
                            if is_appindicator_panic_message(message) {
                                eprintln!(
                                    "the system tray library is missing — install libayatana-appindicator3-1"
                                );
                            }
                        }
                    }));
                }

                let tray = TrayIconBuilder::with_id("main")
                    .icon(match initial_presentation.icon {
                        runtime::TrayIcon::Active => active_icon.clone(),
                        runtime::TrayIcon::Inactive | runtime::TrayIcon::Waiting => {
                            inactive_icon.clone()
                        }
                    })
                    .tooltip(&initial_presentation.tooltip)
                    .menu(&menu)
                    .show_menu_on_left_click(true)
                    .on_menu_event(move |app, event| {
                        use owner::ConfigOrigin;

                        let jiggle_mode = match event.id().as_ref() {
                            "mode_power" => {
                                Some(JiggleMode::PowerOnly)
                            }
                            "mode_subtle" => {
                                Some(JiggleMode::MouseSubtle)
                            }
                            "mode_zen" => {
                                Some(JiggleMode::MouseZen)
                            }
                            "mode_circle" => {
                                Some(JiggleMode::MouseCircle)
                            }
                            _ => None,
                        };
                        if let Some(jiggle_mode) = jiggle_mode {
                            if let Some(runtime) = app.try_state::<owner::RuntimeHandle>() {
                                let runtime = runtime.inner().clone();
                                std::mem::drop(tauri::async_runtime::spawn_blocking(move || {
                                    let pending = runtime.update_config(ConfigOrigin::Other,
                                        |config| {
                                            config.jiggle_mode = jiggle_mode;
                                            Ok(())
                                        }, |_, _| {});
                                    let result = pending
                                        .and_then(|pending| pending.wait(owner::REQUEST_TIMEOUT))
                                        .and_then(|reply| reply.error.map_or(Ok(()), Err));
                                    if let Err(error) = result {
                                        log::error!("Jiggle mode update failed: {error}");
                                    }
                                }));
                            }
                            return;
                        }

                        match event.id().as_ref() {
                            "toggle_active" => {
                                if let Some(runtime) = app.try_state::<owner::RuntimeHandle>() {
                                    if let Err(error) = runtime.post(TrayRoute::TrayToggle) {
                                        log::error!(
                                            "Engine toggle could not reach runtime: {error}"
                                        );
                                    }
                                }
                            }
                            "grant_accessibility" => {
                                // On Linux there is no OS permission dialog to trigger — the
                                // gating condition is whether `xdotool` is present on `PATH`,
                                // which the user can only fix from a terminal. Calling
                                // `request_accessibility()` there is a no-op; clicking this
                                // item is a genuine re-check instead.
                                #[cfg(not(target_os = "linux"))]
                                {
                                    if let Err(e) = perm_for_tray.request_accessibility() {
                                        log::error!("Failed to request accessibility: {}", e);
                                    }
                                }
                                // Re-check — on Linux this is the sole action; on
                                // macOS/Windows it re-checks after the user (potentially)
                                // grants access via the OS dialog opened above.
                                if perm_for_tray.check_accessibility() {
                                    let _ = accessibility_clone.set_text(&acc_ok_text);
                                    let _ = accessibility_clone.set_enabled(false);
                                }
                            }
                            "settings" => {
                                show_settings_window(app);
                            }
                            "about" => {
                                show_about_window(app);
                            }
                            "quit" => {
                                if let Err(error) = request_app_exit(app, 0) {
                                    log::error!("Failed to request app exit: {error}");
                                }
                            }
                            _ => {}
                        }
                    })
                    .build(app)?;

                let sink = NativeTraySink::new(
                    app.handle().clone(),
                    NativeTrayTargets {
                        tray,
                        status_item: toggle_item.clone(),
                        images: TrayImages {
                            active: active_icon,
                            inactive: inactive_icon.clone(),
                            waiting: inactive_icon,
                        },
                    },
                );
                sink.apply_initial(&initial_presentation)?;
                let runtime = owner::RuntimeHandle::spawn(owner::OwnerStart {
                    config: Arc::clone(&shared_config),
                    bundle: Arc::clone(&shared_bundle),
                    clock: Arc::clone(&clock),
                    backend_factory: Box::new(|config, clock| {
                        Box::new(Engine::new_with_clock(
                            platform::create_mouse_driver(),
                            platform::create_power_inhibitor(),
                            Arc::new(Mutex::new(config)),
                            clock,
                        ))
                    }),
                    sink: Box::new(sink),
                })?;
                app.manage(runtime.clone());
                let post_spawn_steps: Vec<PostSpawnStep<'_>> = vec![Box::new(|| {
                        use tauri_plugin_global_shortcut::{
                            Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState,
                        };

                        let shortcut = parse_shortcut_string(&loaded_config.global_hotkey)
                            .unwrap_or_else(|| {
                                log::warn!(
                                    "Could not parse hotkey \"{}\"; using default CmdOrCtrl+Shift+J",
                                    loaded_config.global_hotkey
                                );
                                #[cfg(target_os = "macos")]
                                let modifier = Modifiers::META;
                                #[cfg(not(target_os = "macos"))]
                                let modifier = Modifiers::CONTROL;
                                Shortcut::new(Some(modifier | Modifiers::SHIFT), Code::KeyJ)
                            });

                        app.handle()
                            .plugin(
                                tauri_plugin_global_shortcut::Builder::new()
                                    .with_handler(move |app, _shortcut, event| {
                                        if matches!(event.state(), ShortcutState::Pressed) {
                                            if let Some(runtime) =
                                                app.try_state::<owner::RuntimeHandle>()
                                            {
                                                if let Err(error) = runtime.post(TrayRoute::Hotkey) {
                                                    log::error!(
                                                        "Hotkey could not reach runtime: {error}"
                                                    );
                                                }
                                            }
                                        }
                                    })
                                    .build(),
                            )
                            .map_err(|error| {
                                format!("failed to install global shortcut plugin: {error}")
                            })?;
                        app.global_shortcut().register(shortcut).map_err(|error| {
                            format!("failed to register global shortcut: {error}")
                        })?;
                        app.handle()
                            .plugin(tauri_plugin_autostart::init(
                                tauri_plugin_autostart::MacosLauncher::AppleScript,
                                None,
                            ))
                            .map_err(|error| format!("failed to install autostart plugin: {error}"))?;

                        let wifi_enabled = shared_config
                            .lock()
                            .map_err(|error| format!("Config lock: {error}"))?
                            .wifi_enabled();
                        if wifi_enabled {
                            let mut monitor = shared_wifi_monitor
                                .lock()
                                .map_err(|error| format!("Monitor lock: {error}"))?;
                            if let Err(error) = monitor.start() {
                                log::warn!("WiFi monitor startup failed: {error}");
                            }
                        }
                        Ok(())
                    })];
                let setup_result = run_post_spawn_steps(
                    &runtime,
                    post_spawn_steps,
                    |runtime| runtime.shutdown(owner::SHUTDOWN_TIMEOUT),
                );
                if let Err(error) = setup_result {
                    return Err(error.into());
                }
            }

            log::info!("Stay Awake initialised");
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(
            #[allow(unused_variables)]
            |app, event| {
                match &event {
                    tauri::RunEvent::ExitRequested { api, code, .. } => {
                        if code.is_none() {
                            api.prevent_exit();
                        }
                        #[cfg(desktop)]
                        if code.is_some() {
                            match app.try_state::<owner::RuntimeHandle>() {
                                Some(runtime) if runtime.exit_ready() => {}
                                Some(_) => {
                                    api.prevent_exit();
                                    if let Err(error) = request_app_exit(app, code.unwrap_or(0)) {
                                        log::error!("Failed to request app exit: {error}");
                                    }
                                }
                                None => {
                                    api.prevent_exit();
                                    log::error!("Exit requested before runtime initialization");
                                }
                            }
                        }
                    }
                    #[cfg(desktop)]
                    tauri::RunEvent::Exit
                        if app
                            .try_state::<owner::RuntimeHandle>()
                            .is_some_and(|runtime| !runtime.exit_ready()) =>
                    {
                        log::error!("Application exited before runtime shutdown completed");
                    }
                    #[cfg(target_os = "macos")]
                    tauri::RunEvent::Ready => {
                        // Set Accessory policy AFTER Tauri finishes setup,
                        // otherwise Tauri overrides it during initialization.
                        configure_macos_app();
                    }
                    #[cfg(target_os = "macos")]
                    tauri::RunEvent::WindowEvent {
                        event: tauri::WindowEvent::Destroyed,
                        ..
                    } => {
                        // When all windows are closed, revert to Accessory (hide from Dock).
                        let has_visible_windows = app
                            .webview_windows()
                            .values()
                            .any(|w| w.is_visible().unwrap_or(false));
                        if !has_visible_windows {
                            set_macos_accessory_app();
                        }
                    }
                    _ => {}
                }
            },
        );
}

// @spec-handoff
// Interface: `fn is_appindicator_panic_message(message: &str) -> bool`
// Purpose: pure, testable predicate extracted from the Linux panic-hook diagnostic
//   (E4/D2) that decides whether a panic payload's message text refers to the
//   appindicator dynamic-library load failure raised by `libappindicator-sys`.
// Behavior:
//   - Case-sensitive substring match on `"appindicator"` (matches both
//     `ayatana-appindicator3` and `appindicator3` shapes, since both contain the substring).
//   - Returns `false` for unrelated panic messages.
// Edge cases:
//   - The real `libappindicator-sys` panic payload is a `String` built via format args
//     (not a `&'static str` literal) — the predicate itself is payload-type-agnostic
//     (it takes an already-downcast `&str`), but the caller (Task 2) must downcast
//     `String` first, `&str` second, per the RFC's verified payload shape. Covered by
//     `matches_when_payload_is_a_string_not_a_literal` below, which builds the input via
//     `format!()` to avoid the literal being folded into a `&'static str` by the compiler.
#[cfg(test)]
mod appindicator_panic_hook_tests {
    use super::*;

    #[test]
    fn matches_real_libappindicator_sys_panic_message() {
        let payload = "Failed to load ayatana-appindicator3 or appindicator3 dynamic library";
        assert!(is_appindicator_panic_message(payload));
    }

    #[test]
    fn does_not_match_unrelated_panic_message() {
        let payload = "index out of bounds: the len is 3 but the index is 5";
        assert!(!is_appindicator_panic_message(payload));
    }

    #[test]
    fn matches_when_payload_is_a_string_not_a_literal() {
        // Built via format! so the compiler can't fold this into a &'static str —
        // mirrors the real payload shape the RFC verified (a heap-allocated String
        // from libappindicator-sys's format-args panic, not a string literal).
        let lib_name = "ayatana-appindicator3";
        let payload: String = format!(
            "Failed to load {} or appindicator3 dynamic library",
            lib_name
        );
        assert!(is_appindicator_panic_message(payload.as_str()));
    }
}

// @spec-handoff
// Interface:
//   `fn warn_key_for(is_linux: bool) -> &'static str`
//   `fn ok_key_for(is_linux: bool) -> &'static str`
//   `fn accessibility_warn_key() -> &'static str` (thin `#[cfg(target_os = "linux")]` wrapper
//     over `warn_key_for`)
//   `fn accessibility_ok_key() -> &'static str` (thin `#[cfg(target_os = "linux")]` wrapper
//     over `ok_key_for`)
// Purpose: select the correct Fluent key id for the tray accessibility-warning menu item
//   depending on target OS, so macOS/Windows keep their existing "Grant Accessibility"
//   semantic copy while Linux gets copy naming the real condition (missing `xdotool`).
// Behavior:
//   - `warn_key_for(true)` (Linux) returns `"tray-xdotool-missing"`.
//   - `warn_key_for(false)` (macOS/Windows) returns `"tray-grant-accessibility"`.
//   - `ok_key_for(true)` (Linux) returns `"tray-xdotool-ok"`.
//   - `ok_key_for(false)` (macOS/Windows) returns `"tray-accessibility-ok"`.
// Edge cases:
//   - The pure `_for(bool)` functions carry the branch logic and are unit-tested
//     unconditionally (no `#[cfg(target_os = ...)]` gate on the test module itself), per the
//     step spec's instruction to keep the decision testable without a Linux-only test gate.
//   - The real `#[cfg(target_os = "linux")]`-gated `accessibility_warn_key`/
//     `accessibility_ok_key` wrappers are exercised indirectly by the tray call sites; they
//     are not unit-tested directly since their body is a one-line delegation with no branch.
#[cfg(test)]
mod accessibility_key_selection_tests {
    use super::*;

    #[test]
    fn warn_key_for_linux_is_xdotool_missing() {
        assert_eq!(warn_key_for(true), "tray-xdotool-missing");
    }

    #[test]
    fn warn_key_for_non_linux_is_grant_accessibility() {
        assert_eq!(warn_key_for(false), "tray-grant-accessibility");
    }

    #[test]
    fn ok_key_for_linux_is_xdotool_ok() {
        assert_eq!(ok_key_for(true), "tray-xdotool-ok");
    }

    #[test]
    fn ok_key_for_non_linux_is_accessibility_ok() {
        assert_eq!(ok_key_for(false), "tray-accessibility-ok");
    }
}

#[cfg(test)]
mod core_helper_tests {
    /*
     * @spec-handoff
     * @interface parse_shortcut_string(), create_dimmed_icon(), is_within_schedule()
     * @behavior
     *   - Shortcut aliases produce the same registered shortcut and invalid parts are rejected.
     *   - A dimmed icon preserves RGB bytes and scales each alpha byte to 40 percent.
     *   - Disabled schedules do not permit scheduled autostart.
     * @edge-cases
     *   - Empty or unknown shortcut parts are invalid.
     */
    use super::*;

    #[cfg(desktop)]
    #[test]
    fn shortcut_parser_accepts_aliases_and_rejects_unknown_parts() {
        use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};

        let cmd_or_ctrl = if cfg!(target_os = "macos") {
            Modifiers::SUPER
        } else {
            Modifiers::CONTROL
        };
        assert_eq!(
            parse_shortcut_string("CmdOrCtrl + Shift + J"),
            Some(Shortcut::new(
                Some(cmd_or_ctrl | Modifiers::SHIFT),
                Code::KeyJ
            ))
        );
        assert_eq!(parse_shortcut_string("Ctrl+Hyper+J"), None);
        assert_eq!(parse_shortcut_string(""), None);
    }

    #[test]
    fn dimmed_icon_preserves_rgb_and_scales_alpha() {
        let icon = tauri::image::Image::new_owned(vec![1, 2, 3, 255, 4, 5, 6, 100], 2, 1);
        let dimmed = create_dimmed_icon(&icon);
        assert_eq!(dimmed.rgba(), &[1, 2, 3, 102, 4, 5, 6, 40]);
    }

    #[test]
    fn disabled_schedule_does_not_trigger_scheduled_autostart() {
        let config = AppConfig {
            schedule_enabled: false,
            ..AppConfig::default()
        };
        assert!(!is_within_schedule(&config));
    }
}

/*
 * @spec-handoff
 * @interface NativeTraySink::{apply_initial,publish}; request_app_exit
 * @interface async fn {toggle_engine,get_engine_state,save_config,load_profile,set_language}
 * @interface fn {save_profile,delete_profile,update_global_hotkey}; tray jiggle-mode setters
 * @behavior
 *   - Native publication keeps only the latest pending frame and schedules UI work once.
 *   - Tauri callbacks submit owner work without directly controlling Engine or waiting on UI.
 *   - Config writers submit update_config through the owner instead of directly locking config.
 *   - Setup paints actual pre-start state, installs Wi-Fi forwarding before monitoring, and
 *     allows process exit only after owner shutdown confirms completion.
 * @edge-cases
 *   - Unchanged ticks coalesce; explicit frames retry; enqueue failure clears scheduling state.
 *   - code=None remains close-to-tray and tray clicks do not toggle activity.
 */
#[cfg(test)]
mod native_wiring_contract_tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    struct FakeUiExecutor {
        tasks: Mutex<VecDeque<Box<dyn FnOnce() + Send>>>,
        fail_next_enqueue: AtomicBool,
    }

    impl FakeUiExecutor {
        fn run_next(&self) {
            self.tasks
                .lock()
                .expect("fake UI task lock")
                .pop_front()
                .expect("a UI task is queued")();
        }

        fn queued(&self) -> usize {
            self.tasks.lock().expect("fake UI task lock").len()
        }
    }

    impl UiExecutor for FakeUiExecutor {
        fn run_on_main_thread(&self, task: Box<dyn FnOnce() + Send>) -> Result<(), String> {
            if self.fail_next_enqueue.swap(false, Ordering::AcqRel) {
                return Err("enqueue failed".to_owned());
            }
            self.tasks
                .lock()
                .expect("fake UI task lock")
                .push_back(task);
            Ok(())
        }
    }

    struct FakePaintTargets {
        frames: Mutex<Vec<TrayPresentation>>,
        fail_next_paint: AtomicBool,
    }

    impl TrayPaintTargets for FakePaintTargets {
        fn paint(&self, presentation: &TrayPresentation) -> Result<(), String> {
            self.frames
                .lock()
                .expect("fake paint lock")
                .push(presentation.clone());
            if self.fail_next_paint.swap(false, Ordering::AcqRel) {
                Err("paint failed".to_owned())
            } else {
                Ok(())
            }
        }
    }

    fn fake_presentation(text: &str) -> TrayPresentation {
        TrayPresentation {
            status: runtime::TrayStatus::Inactive,
            menu_text: text.to_owned(),
            tooltip: text.to_owned(),
            icon: runtime::TrayIcon::Inactive,
        }
    }

    #[test]
    fn native_sink_recovers_from_enqueue_failure_and_coalesces_deferred_paints() {
        let executor = Arc::new(FakeUiExecutor {
            tasks: Mutex::new(VecDeque::new()),
            fail_next_enqueue: AtomicBool::new(true),
        });
        let targets = Arc::new(FakePaintTargets {
            frames: Mutex::new(Vec::new()),
            fail_next_paint: AtomicBool::new(true),
        });
        let mut sink = NativeTraySink::new_with_ui(executor.clone(), targets.clone());
        let a = fake_presentation("A");
        let b = fake_presentation("B");
        let c = fake_presentation("C");

        assert_eq!(
            sink.publish(TrayRoute::Tick, &a),
            Err("enqueue failed".to_owned())
        );
        assert_eq!(
            executor.queued(),
            0,
            "a failed enqueue leaves no stale callback"
        );
        sink.publish(TrayRoute::Tick, &a)
            .expect("a later frame reschedules after enqueue failure");
        sink.publish(TrayRoute::Tick, &a)
            .expect("an unchanged tick is coalesced");
        sink.publish(TrayRoute::TrayToggle, &b)
            .expect("an explicit frame supersedes the pending tick");
        assert_eq!(
            executor.queued(),
            1,
            "publish returns without waiting for UI completion"
        );

        executor.run_next();
        assert_eq!(
            targets.frames.lock().expect("fake paint lock").as_slice(),
            std::slice::from_ref(&b),
            "only the latest queued frame is painted; setter failures do not reject it"
        );

        sink.publish(TrayRoute::Tick, &c)
            .expect("a publication after painting queues another callback");
        assert_eq!(
            executor.queued(),
            1,
            "a frame published after paint is not lost"
        );
        executor.run_next();
        assert_eq!(
            targets.frames.lock().expect("fake paint lock").as_slice(),
            &[b, c],
            "the later frame is painted after the earlier callback"
        );
    }

    fn without_comments(source: &str) -> String {
        let bytes = source.as_bytes();
        let mut output = String::with_capacity(source.len());
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index..].starts_with(b"//") {
                index += 2;
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            } else if bytes[index..].starts_with(b"/*") {
                index += 2;
                while index + 1 < bytes.len() && !bytes[index..].starts_with(b"*/") {
                    index += 1;
                }
                index = (index + 2).min(bytes.len());
            } else {
                output.push(bytes[index] as char);
                index += 1;
            }
        }
        output
    }

    fn scoped_block<'a>(source: &'a str, anchor: &str) -> &'a str {
        let start = source
            .find(anchor)
            .unwrap_or_else(|| panic!("missing scope anchor: {anchor}"));
        let open = source[start..]
            .find('{')
            .map(|offset| start + offset)
            .unwrap_or_else(|| panic!("missing opening brace for: {anchor}"));
        let mut depth = 0usize;
        for (offset, byte) in source[open..].bytes().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &source[start..=open + offset];
                    }
                }
                _ => {}
            }
        }
        panic!("unclosed scope: {anchor}");
    }

    fn handler_table(source: &str) -> &str {
        let start = source
            .find("tauri::generate_handler![")
            .expect("generate_handler macro exists");
        let open = source[start..]
            .find('[')
            .map(|offset| start + offset)
            .expect("generate_handler opens its entries");
        let close = source[open..]
            .find("])")
            .map(|offset| open + offset)
            .expect("generate_handler closes its entries");
        &source[open..=close]
    }

    fn source() -> String {
        without_comments(include_str!("lib.rs"))
    }

    fn production_source() -> String {
        let source = source();
        let test_module = source
            .find("#[cfg(test)]\nmod native_wiring_contract_tests")
            .expect("native wiring tests remain separate from production source");
        source[..test_module].to_owned()
    }

    #[test]
    fn native_sink_publishes_latest_frames_on_the_main_thread_without_waiting() {
        let source = source();
        let native_impl = scoped_block(&source, "impl NativeTraySink {");
        let publish_impl = scoped_block(&source, "impl TraySink for NativeTraySink {");

        assert!(
            native_impl.contains("fn apply_initial(&self, presentation: &TrayPresentation)"),
            "initial paint accepts the production presentation"
        );
        assert!(
            native_impl.contains("status_item.set_text"),
            "initial paint updates the status item from its presentation"
        );
        assert!(
            publish_impl.contains("last_requested"),
            "unchanged Tick frames are coalesced against the requested frame"
        );
        assert!(
            publish_impl.contains("run_on_main_thread"),
            "publication schedules native setters on the UI thread"
        );
        assert!(
            publish_impl.contains("scheduled = false"),
            "an enqueue failure makes a later publication schedulable"
        );
        assert!(
            publish_impl.contains("log::error!"),
            "setter failures are logged after the owner has accepted the frame"
        );
        assert!(
            !publish_impl.contains("recv_timeout") && !publish_impl.contains(".wait("),
            "owner publication never waits for native UI completion"
        );
    }

    #[test]
    fn setup_uses_actual_initial_presentation_and_installs_wifi_listener_before_monitor_start() {
        let source = source();
        let setup = scoped_block(&source, ".setup(|app| {");
        let initial = scoped_block(&source, "pub(crate) fn initial_runtime_snapshot(");

        assert!(
            initial.contains("engine_active: false"),
            "first presentation derives from confirmed pre-start inactivity"
        );
        assert!(
            setup.contains("initial_runtime_snapshot"),
            "setup builds a presentation from actual initial inputs"
        );
        assert!(
            !setup.contains(".icon(active_icon)") && !setup.contains(".tooltip(&tip_active)"),
            "tray creation does not hard-code an active presentation"
        );
        let listener = setup
            .find("app.listen(\"wifi-state-changed\"")
            .expect("Wi-Fi listener exists");
        let monitor_start = setup
            .find("monitor.start()")
            .expect("Wi-Fi monitor startup exists");
        assert!(
            listener < monitor_start,
            "the Wi-Fi listener is installed before the monitor can emit"
        );
    }

    #[test]
    fn setup_shuts_down_the_owner_before_propagating_a_post_spawn_setup_error() {
        let source = source();
        let setup = scoped_block(&source, ".setup(|app| {");
        let spawn = setup
            .find("owner::RuntimeHandle::spawn")
            .expect("setup starts the owner");
        let post_spawn = &setup[spawn..];

        assert!(
            post_spawn.contains("shutdown(owner::SHUTDOWN_TIMEOUT)"),
            "every fallible setup step after owner spawn must use bounded owner cleanup"
        );
    }

    #[test]
    fn ipc_commands_are_desktop_async_owner_requests_with_bounded_blocking_waits() {
        let source = source();
        let handlers = handler_table(&source);

        for command in [
            "toggle_engine",
            "get_engine_state",
            "save_config",
            "load_profile",
            "set_language",
        ] {
            assert!(
                handlers.contains(&format!("#[cfg(desktop)]\n            {command},")),
                "generate_handler registers {command} rather than its legacy handler"
            );
        }
        for legacy in [
            "legacy_toggle_engine",
            "legacy_get_engine_state",
            "legacy_save_config",
            "legacy_load_profile",
            "legacy_set_language",
        ] {
            assert!(
                !handlers.contains(&format!("            {legacy},")),
                "generate_handler no longer exposes {legacy}"
            );
        }
        let toggle = scoped_block(&source, "async fn toggle_engine(");
        let query = scoped_block(&source, "async fn get_engine_state(");
        let save = scoped_block(&source, "async fn save_config(");
        let profile = scoped_block(&source, "async fn load_profile(");
        let language = scoped_block(&source, "async fn set_language(");
        for body in [toggle, query, save, profile, language] {
            assert!(
                body.contains("spawn_blocking"),
                "owner IPC waits run on Tauri's blocking-task helper"
            );
            assert!(
                body.contains("REQUEST_TIMEOUT"),
                "owner IPC waits use the documented timeout"
            );
            assert!(
                !body.contains("Engine") && !body.contains(".start(") && !body.contains(".stop("),
                "IPC handlers do not directly control the engine"
            );
        }
        assert!(toggle.contains("runtime.request(TrayRoute::IpcToggle)"));
        assert!(query.contains("runtime.query()"));
        assert!(save.contains("runtime.update_config(ConfigOrigin::Settings"));
        assert!(profile.contains("runtime.update_config(ConfigOrigin::Profile"));
        assert!(language.contains("runtime.update_config(ConfigOrigin::Language"));
    }

    #[test]
    fn callbacks_forward_to_the_owner_without_direct_engine_control_or_extra_click_toggle() {
        let source = source();
        let setup = scoped_block(&source, ".setup(|app| {");
        let menu = scoped_block(&source, ".on_menu_event(move |app, event| {");
        let shortcut = scoped_block(&source, ".with_handler(move |app, _shortcut, event| {");
        let wifi = scoped_block(&source, "app.listen(\"wifi-state-changed\"");
        let production = production_source();

        assert!(
            menu.contains("runtime.post(TrayRoute::TrayToggle)"),
            "tray menu toggle submits its named owner route"
        );
        assert!(
            shortcut.contains("runtime.post(TrayRoute::Hotkey)"),
            "pressed hotkey submits its named owner route"
        );
        assert!(
            wifi.contains("runtime.post_wifi(owner::WifiObservation { ssid: payload.ssid })"),
            "Wi-Fi listener forwards SSID rather than a stale active flag"
        );
        for body in [menu, shortcut, wifi] {
            assert!(
                !body.contains(".start(") && !body.contains(".stop(") && !body.contains(".toggle("),
                "callbacks do not bypass the runtime owner"
            );
        }
        assert!(
            !setup.contains("on_tray_icon_event"),
            "left click opens the menu without an additional activity toggle"
        );
        assert!(
            !production.contains("eng.start(")
                && !production.contains("eng.stop(")
                && !production.contains("eng.toggle("),
            "lib.rs leaves engine lifecycle operations to the owner path"
        );
    }

    #[test]
    fn save_profile_updates_config_through_the_owner_transaction() {
        let source = source();
        let handler = scoped_block(&source, "fn save_profile(");

        assert!(
            handler.contains("runtime.update_config(ConfigOrigin::Other"),
            "saving a profile commits its config metadata through the runtime owner"
        );
        assert!(
            !handler.contains("config.lock()")
                && !handler.contains("*cfg =")
                && !handler.contains("cfg.save()"),
            "saving a profile does not mutate the shared config directly"
        );
    }

    #[test]
    fn delete_profile_updates_config_through_the_owner_transaction() {
        let source = source();
        let handler = scoped_block(&source, "fn delete_profile(");

        assert!(
            handler.contains("runtime.update_config(ConfigOrigin::Other"),
            "deleting a profile commits its config metadata through the runtime owner"
        );
        assert!(
            !handler.contains("config.lock()")
                && !handler.contains("*cfg =")
                && !handler.contains("cfg.save()"),
            "deleting a profile does not mutate the shared config directly"
        );
    }

    #[test]
    fn update_global_hotkey_commits_through_the_owner_transaction() {
        let source = source();
        let handler = scoped_block(&source, "fn update_global_hotkey(");

        assert!(
            handler.contains("runtime.update_config(ConfigOrigin::Other"),
            "updating the hotkey commits config through the runtime owner"
        );
        assert!(
            !handler.contains("config.lock()")
                && !handler.contains("*cfg =")
                && !handler.contains("cfg.save()"),
            "updating the hotkey does not mutate the shared config directly"
        );
    }

    #[test]
    fn tray_jiggle_mode_setters_update_config_through_the_owner_transaction() {
        let source = source();
        let handler = scoped_block(&source, ".on_menu_event(move |app, event| {");

        for (setter, mode) in [
            ("mode_power", "JiggleMode::PowerOnly"),
            ("mode_subtle", "JiggleMode::MouseSubtle"),
            ("mode_zen", "JiggleMode::MouseZen"),
            ("mode_circle", "JiggleMode::MouseCircle"),
        ] {
            let setter = scoped_block(handler, &format!("\"{setter}\" => {{"));
            assert!(
                setter.contains(mode),
                "tray jiggle-mode setter maps its menu id to {mode}"
            );
        }
        assert!(
            handler
                .matches("runtime.update_config(ConfigOrigin::Other")
                .count()
                >= 1,
            "tray jiggle-mode setters commit through the shared owner transaction"
        );
        assert!(
            !handler.contains("config.lock()")
                && !handler.contains("*cfg =")
                && !handler.contains("cfg.save()"),
            "tray jiggle-mode setters do not mutate the shared config directly"
        );
    }

    #[test]
    fn exit_is_coordinated_after_owner_completion_and_window_close_stays_in_tray() {
        let source = source();
        let request_exit = scoped_block(&source, "pub(crate) fn request_app_exit(");
        let run_events = scoped_block(&source, ".run(");
        let menu = scoped_block(&source, ".on_menu_event(move |app, event| {");

        assert!(
            menu.contains("request_app_exit(app, 0)"),
            "Quit delegates shutdown to the exit coordinator"
        );
        assert!(
            request_exit.contains("claim_exit_coordinator")
                && request_exit.contains("runtime.shutdown(SHUTDOWN_TIMEOUT)")
                && request_exit.contains("app.exit(code)"),
            "only a claimed coordinator shuts down the owner before app exit"
        );
        assert!(
            run_events.contains("if code.is_none()") && run_events.contains("api.prevent_exit()"),
            "window close remains a close-to-tray event"
        );
        assert!(
            run_events.contains("runtime.exit_ready()") && run_events.contains("request_app_exit"),
            "ExitRequested prevents early process exit until owner completion"
        );
        assert!(
            run_events.contains("Some(runtime) if runtime.exit_ready() => {}"),
            "only an exit-ready owner permits the requested process exit"
        );
        assert!(
            run_events.contains("tauri::RunEvent::Exit") && run_events.contains("exit_ready"),
            "final Exit checks/logs unexpected shutdown before process teardown"
        );
    }
}
