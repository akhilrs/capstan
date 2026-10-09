//! Keeps one Supervisor running while workers are active and queues it a routine check (src/supervision.ts). It does
//! nothing without an active PM, a configured Supervisor role and `supervision.enabled`. A failed spawn is logged and
//! tried again after a minute; a Supervisor is released after ten minutes without any active worker.

use super::driver::Steps;
use super::notifier::Clock;
use capstan_launcher::api::{LauncherService, SpawnOptions};
use serde_json::{json, Value};
use std::sync::Arc;

/// How long no worker may have been active before the controller releases its Supervisor.
pub const SUPERVISOR_IDLE_MS: i64 = 10 * 60 * 1000;
const SPAWN_RETRY_MS: i64 = 60 * 1000;

pub struct SupervisionOptions {
    pub steps: Steps,
    pub launcher: Arc<dyn LauncherService>,
    pub enabled: bool,
    pub check_seconds: f64,
    pub supervisor_role: Option<String>,
    pub now: Clock,
    pub log: super::driver::DriverLog,
}

pub struct Supervision {
    options: SupervisionOptions,
    last_worker_seen: i64,
    retry_at: i64,
    release_retry_at: i64,
}

impl Supervision {
    pub fn new(options: SupervisionOptions) -> Self {
        let last_worker_seen = (options.now)();
        Self {
            options,
            last_worker_seen,
            retry_at: 0,
            release_retry_at: 0,
        }
    }

    fn log(&self, event: &str, details: Value) {
        (self.options.log)(event, details);
    }

    fn now(&self) -> i64 {
        (self.options.now)()
    }

    /// One look; a failure is logged as `supervision_failed`.
    pub fn tick(&mut self) {
        if let Err(error) = self.tick_once() {
            self.log("supervision_failed", json!({"error": error}));
        }
    }

    fn tick_once(&mut self) -> Result<(), String> {
        let Some(role) = self.options.supervisor_role.clone() else {
            return Ok(());
        };
        if !self.options.enabled {
            return Ok(());
        }
        let agents = match self
            .options
            .steps
            .read("activeAgents", |core| core.active_agents())
            .map_err(|e| e.to_string())?
        {
            Value::Array(agents) => agents,
            _ => Vec::new(),
        };
        let of_kind = |kinds: &[&str]| -> Vec<&Value> {
            agents
                .iter()
                .filter(|a| kinds.contains(&a["kind"].as_str().unwrap_or("")))
                .collect()
        };
        if of_kind(&["PM"]).len() != 1 {
            return Ok(());
        }
        let workers = of_kind(&["Developer", "Verifier"]).len();
        let supervisors: Vec<String> = of_kind(&["Supervisor"])
            .iter()
            .map(|a| a["agentId"].as_str().unwrap_or("").to_string())
            .collect();
        if workers > 0 {
            self.last_worker_seen = self.now();
        }
        if workers > 0 && supervisors.is_empty() && self.now() >= self.retry_at {
            match self.options.launcher.spawn(&role, &SpawnOptions::default()) {
                Ok(result) => self.log("supervisor_started", json!({"state": result.state})),
                Err(error) => {
                    self.retry_at = self.now() + SPAWN_RETRY_MS;
                    self.log(
                        "supervisor_spawn_failed",
                        json!({"error": format!("LauncherError: {}", error.message)}),
                    );
                }
            }
            return Ok(());
        }
        if supervisors.len() == 1 {
            if workers == 0
                && self.now() - self.last_worker_seen >= SUPERVISOR_IDLE_MS
                && self.now() >= self.release_retry_at
            {
                match self.options.launcher.release(&supervisors[0]) {
                    Ok(_) => self.log("supervisor_released", json!({"agentId": supervisors[0]})),
                    Err(error) => {
                        self.release_retry_at = self.now() + SPAWN_RETRY_MS;
                        self.log(
                            "supervisor_release_failed",
                            json!({"error": format!("LauncherError: {}", error.message)}),
                        );
                    }
                }
                return Ok(());
            }
            let seconds = self.options.check_seconds;
            let queued = self
                .options
                .steps
                .mutate("queueSupervisionCheck", move |core, context| {
                    core.queue_supervision_check(context, seconds)
                })
                .map_err(|e| e.to_string())?;
            if queued["queued"].as_bool().unwrap_or(false) {
                self.log(
                    "supervision_check_queued",
                    json!({"cancelled": queued["cancelled"]}),
                );
            }
        }
        Ok(())
    }
}
