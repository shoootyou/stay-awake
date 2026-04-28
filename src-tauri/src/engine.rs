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

    // Check day of week
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

/// Engine operational state.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineState {
    /// Engine is not running.
    Idle,
    /// Engine is actively preventing inactivity.
    Active,
    /// Engine is temporarily paused (reserved for future use).
    #[allow(dead_code)]
    Paused,
    /// Engine encountered an error (reserved for future use).
    #[allow(dead_code)]
    Error(String),
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
            power.inhibit_sleep("No Sleep Please!: keeping system awake")?;
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

                // ── Idle detection ──────────────────────────────────────────
                // If the cursor moved since the last check the user is active
                // and we skip the jiggle to avoid interfering.
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

                // ── Execute the configured jiggle pattern ───────────────────
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
            EngineState::Paused => "paused",
            EngineState::Error(_) => "error",
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
