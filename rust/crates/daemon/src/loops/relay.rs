//! Queues the PM notice for accepted reports and finished reviews that have none, on every tick (`startReportRelay` of
//! src/reports.ts). Safe to run more than once: the notice is queued once per report.

use super::driver::Steps;
use super::notifier::Clock;
use capstan_kernel::types::MutationContext;
use serde_json::{json, Value};
use std::cell::RefCell;

const RELAY_BACKOFF_MS: i64 = 30_000;

pub struct RelayOptions {
    pub steps: Steps,
    pub credential: String,
    pub now: Clock,
    /// When set, open findings with no check by their Supervisor this long after the latest delivery are escalated on
    /// each tick.
    pub finding_check_seconds: Option<f64>,
    pub log: super::driver::DriverLog,
}

pub struct Relay {
    options: RelayOptions,
    backoff_until: i64,
    /// The ledger version at which the last full pass found nothing to announce: until a mutation moves it, a pass would
    /// find nothing again.
    quiet_at_version: i64,
}

impl Relay {
    pub fn new(options: RelayOptions) -> Self {
        Self {
            options,
            backoff_until: 0,
            quiet_at_version: -1,
        }
    }

    fn log(&self, event: &str, details: Value) {
        (self.options.log)(event, details);
    }

    pub fn tick(&mut self) {
        if (self.options.now)() < self.backoff_until {
            return;
        }
        if let Err(error) = self.pass() {
            self.log("report_relay_failed", json!({"error": error}));
        }
    }

    fn array(value: Value) -> Vec<Value> {
        match value {
            Value::Array(items) => items,
            _ => Vec::new(),
        }
    }

    fn pass(&mut self) -> Result<(), String> {
        let text = |e: capstan_kernel::KernelError| e.to_string();
        if let Some(seconds) = self.options.finding_check_seconds {
            // Its own handling: a sweep that keeps failing must not stop the announcements below.
            self.sweep(seconds);
        }
        // What is announced depends only on the ledger, and every ledger change moves its version.
        let version = self
            .options
            .steps
            .read("stateVersion", |core| core.state_version().map(Value::from))
            .map_err(text)?
            .as_i64()
            .unwrap_or(-1);
        if version == self.quiet_at_version {
            return Ok(());
        }
        // Nothing to do, and nothing written, until a PM is active. The core announces only when exactly one PM is
        // active; use the same condition.
        let agents = Self::array(
            self.options
                .steps
                .read("activeAgents", |core| core.active_agents())
                .map_err(text)?,
        );
        if agents.iter().filter(|a| a["kind"] == "PM").count() != 1 {
            self.quiet_at_version = version;
            return Ok(());
        }
        let credential = self.options.credential.clone();
        let mut found = false;
        let reports = {
            let credential = credential.clone();
            Self::array(
                self.options
                    .steps
                    .read("unannouncedReports", move |core| {
                        core.unannounced_reports(&credential, None)
                    })
                    .map_err(text)?,
            )
        };
        for report in reports {
            found = true;
            let id = report["reportId"].as_str().unwrap_or("").to_string();
            let announced = self
                .options
                .steps
                .mutate("announceReport", {
                    let id = id.clone();
                    move |core, context| core.announce_report(context, &id)
                })
                .map_err(text)?;
            if !announced["announced"].as_bool().unwrap_or(false) {
                // It could not be announced just now (the PM changed, no controller actor): try again later, not every
                // tick.
                self.backoff_until = (self.options.now)() + RELAY_BACKOFF_MS;
                return Ok(());
            }
            self.log("report_announced", json!({"reportId": id}));
        }
        let reviews = {
            let credential = credential.clone();
            Self::array(
                self.options
                    .steps
                    .read("unannouncedReviews", move |core| {
                        core.unannounced_reviews(&credential, None)
                    })
                    .map_err(text)?,
            )
        };
        for review in reviews {
            found = true;
            let id = review["reviewId"].as_str().unwrap_or("").to_string();
            let announced = self
                .options
                .steps
                .mutate("announceReview", {
                    let id = id.clone();
                    move |core, context| core.announce_review(context, &id)
                })
                .map_err(text)?;
            if !announced["announced"].as_bool().unwrap_or(false) {
                self.backoff_until = (self.options.now)() + RELAY_BACKOFF_MS;
                return Ok(());
            }
            self.log("review_announced", json!({"reviewId": id}));
        }
        let plan_notices = {
            let credential = credential.clone();
            Self::array(
                self.options
                    .steps
                    .read("unannouncedPlanNotices", move |core| {
                        core.unannounced_plan_notices(&credential)
                    })
                    .map_err(text)?,
            )
        };
        for notice in plan_notices {
            found = true;
            let announced = self
                .options
                .steps
                .mutate("announcePlanNotice", {
                    let notice = notice.clone();
                    move |core, context| core.announce_plan_notice(context, &notice)
                })
                .map_err(text)?;
            if !announced["announced"].as_bool().unwrap_or(false) {
                self.backoff_until = (self.options.now)() + RELAY_BACKOFF_MS;
                return Ok(());
            }
            self.log("plan_notice_announced", notice);
        }
        let finding_notices = {
            let credential = credential.clone();
            Self::array(
                self.options
                    .steps
                    .read("unannouncedFindingNotices", move |core| {
                        core.unannounced_finding_notices(&credential, None)
                    })
                    .map_err(text)?,
            )
        };
        for notice in finding_notices {
            found = true;
            let id = notice["noticeId"].as_str().unwrap_or("").to_string();
            let announced = self
                .options
                .steps
                .mutate("announceFindingNotice", move |core, context| {
                    core.announce_finding_notice(context, &id)
                })
                .map_err(text)?;
            // Not announced: raced with another announcer or waiting its turn; the next tick looks again.
            if !announced["announced"].as_bool().unwrap_or(false) {
                break;
            }
            self.log(
                "finding_notice_announced",
                json!({"findingId": notice["findingId"], "event": notice["event"]}),
            );
        }
        if !found {
            self.quiet_at_version = version;
        }
        Ok(())
    }

    fn sweep(&self, seconds: f64) {
        let credential = self.options.credential.clone();
        let steps = self.options.steps.clone();
        let outcome = self.options.steps.read("sweepFindings", move |core| {
            let failed: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());
            let make = || {
                // `newContext` reads the two counters.
                steps.count("stateVersion");
                steps.count("inputRevision");
                crate::deps::new_context(core, &credential).unwrap_or_else(|_| MutationContext {
                    credential: credential.clone(),
                    request_id: String::new(),
                    idempotency_key: String::new(),
                    expected_version: -1,
                    input_revision: -1,
                })
            };
            let record = |id: &str, error: &capstan_kernel::KernelError| {
                failed
                    .borrow_mut()
                    .push((id.to_string(), error.to_string()));
            };
            let escalated = core.sweep_findings(&make, seconds, Some(&record))?;
            Ok(json!({"escalated": escalated, "failed": failed.into_inner()}))
        });
        match outcome {
            Ok(result) => {
                for failure in result["failed"].as_array().into_iter().flatten() {
                    self.log(
                        "finding_deadline_failed",
                        json!({"findingId": failure[0], "error": failure[1]}),
                    );
                }
                for id in result["escalated"].as_array().into_iter().flatten() {
                    self.log("finding_escalated_by_deadline", json!({"findingId": id}));
                }
            }
            Err(error) => self.log("finding_sweep_failed", json!({"error": error.to_string()})),
        }
    }
}
