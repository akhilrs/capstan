//! The resolved configuration as JavaScript values: what `JSON.stringify(roleConfig, null, 2)` prints (key order
//! included) and the canonical JSON `digestJson` hashes (`src/controller/canonical.ts`).
use capstan_wire::js::{self, JsStr, Value};
use sha2::{Digest, Sha256};

use crate::types::{Host, McpServer, PromptSource, Role, RoleConfig};

fn text(value: &str) -> Value {
    Value::String(JsStr::from(value))
}

fn number(value: i64) -> Value {
    Value::Number(value as f64)
}

fn optional(value: &Option<String>) -> Value {
    value.as_deref().map_or(Value::Null, text)
}

fn strings(values: &[String]) -> Value {
    Value::Array(values.iter().map(|v| text(v)).collect())
}

fn object(members: Vec<(&str, Value)>) -> Value {
    Value::Object(
        members
            .into_iter()
            .map(|(key, value)| (JsStr::from(key), value))
            .collect(),
    )
}

pub fn sha256(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// `digestJson`: the SHA-256 of the compact JSON with every object's keys sorted.
pub fn digest_json(value: &Value) -> String {
    sha256(&js::stringify(&sorted(value), 0).to_utf8_lossy())
}

fn sorted(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        Value::Object(members) => {
            let mut members: Vec<(JsStr, Value)> = members
                .iter()
                .map(|(k, v)| (k.clone(), sorted(v)))
                .collect();
            members.sort_by(|a, b| a.0.as_units().cmp(b.0.as_units()));
            Value::Object(members)
        }
        other => other.clone(),
    }
}

fn mcp_server(server: &McpServer) -> Value {
    object(vec![
        ("name", text(&server.name)),
        ("command", text(&server.command)),
        ("args", strings(&server.args)),
    ])
}

fn host(host: &Host) -> Value {
    object(vec![
        ("name", text(&host.name)),
        ("kind", text(&host.kind)),
        ("command", text(&host.command)),
        (
            "shellCommandTimeoutSeconds",
            number(host.shell_command_timeout_seconds),
        ),
        ("waitTimeoutSeconds", number(host.wait_timeout_seconds)),
    ])
}

fn prompt(prompt: &PromptSource) -> Value {
    object(vec![
        ("source", text(prompt.source)),
        ("path", optional(&prompt.path)),
        ("hash", optional(&prompt.hash)),
    ])
}

/// The members of `resolved` in `resolveRoles` (no `configHash`; `mcp` only when the role has servers).
fn resolved_role(role: &Role, prompt_value: Value) -> Vec<(&'static str, Value)> {
    let mut members = vec![
        ("name", text(&role.name)),
        ("kind", text(&role.kind)),
        ("host", text(&role.host)),
        ("model", optional(&role.model)),
        ("permissionMode", text(&role.permission_mode)),
        ("allow", strings(&role.allow)),
        ("deny", strings(&role.deny)),
        ("hooks", text(role.hooks)),
    ];
    if !role.mcp.is_empty() {
        members.push((
            "mcp",
            Value::Array(role.mcp.iter().map(mcp_server).collect()),
        ));
    }
    members.push(("prompt", prompt_value));
    members
}

/// What `configHash` covers: the role with only the prompt's source and hash, and its host.
pub fn role_hash(role: &Role, role_host: &Host) -> String {
    let covered = object(vec![
        ("source", text(role.prompt.source)),
        ("hash", optional(&role.prompt.hash)),
    ]);
    digest_json(&object(vec![
        ("role", object(resolved_role(role, covered))),
        ("host", host(role_host)),
    ]))
}

fn role(role: &Role) -> Value {
    let mut members = resolved_role(role, prompt(&role.prompt));
    if role.mcp.is_empty() {
        members.push(("mcp", Value::Array(Vec::new())));
    }
    members.push(("configHash", text(&role.config_hash)));
    object(members)
}

impl RoleConfig {
    /// The value `JSON.stringify` walks, in the order the loader builds the object.
    pub fn to_value(&self) -> Value {
        let n = &self.notifications;
        let t = &self.timers;
        let o = &self.operator;
        let mut members = vec![
            ("schemaVersion", number(1)),
            ("projectName", optional(&self.project_name)),
            ("herdrSession", text(&self.herdr_session)),
            (
                "notifications",
                object(vec![
                    ("herdr", Value::Bool(n.herdr)),
                    ("fallback", Value::Bool(n.fallback)),
                    ("pmStaleMinutes", number(n.pm_stale_minutes)),
                ]),
            ),
            (
                "timers",
                object(vec![
                    ("maxDeferralSeconds", number(t.max_deferral_seconds)),
                    (
                        "maxBusyDeferralSeconds",
                        number(t.max_busy_deferral_seconds),
                    ),
                    ("pmAckTimeoutSeconds", number(t.pm_ack_timeout_seconds)),
                    ("pmNotifyAfterSeconds", number(t.pm_notify_after_seconds)),
                    ("notifyIntervalSeconds", number(t.notify_interval_seconds)),
                    ("stallAfterSeconds", number(t.stall_after_seconds)),
                    (
                        "workerAckTimeoutSeconds",
                        number(t.worker_ack_timeout_seconds),
                    ),
                    ("findingCheckSeconds", number(t.finding_check_seconds)),
                    ("pmWakeAfterSeconds", number(t.pm_wake_after_seconds)),
                    ("pmWakeIntervalSeconds", number(t.pm_wake_interval_seconds)),
                ]),
            ),
            (
                "supervision",
                object(vec![
                    ("enabled", Value::Bool(self.supervision.enabled)),
                    ("checkSeconds", number(self.supervision.check_seconds)),
                ]),
            ),
            (
                "architect",
                object(vec![
                    ("enabled", Value::Bool(self.architect.enabled)),
                    ("role", text(&self.architect.role)),
                    ("planReview", text(&self.architect.plan_review)),
                    ("reviewerRole", optional(&self.architect.reviewer_role)),
                    ("maxPackages", number(self.architect.max_packages)),
                    (
                        "countTowardWorkerLimit",
                        Value::Bool(self.architect.count_toward_worker_limit),
                    ),
                    (
                        "highRiskTriggers",
                        strings(&self.architect.high_risk_triggers),
                    ),
                ]),
            ),
            (
                "operator",
                object(vec![
                    ("configured", Value::Bool(o.configured)),
                    ("enabled", Value::Bool(o.enabled)),
                    ("role", text(&o.role)),
                    ("autoApprove", strings(&o.auto_approve)),
                    ("autoApprovePrefix", strings(&o.auto_approve_prefix)),
                    ("timeoutSeconds", number(o.timeout_seconds)),
                    ("maxTimeoutSeconds", number(o.max_timeout_seconds)),
                    ("outputTailBytes", number(o.output_tail_bytes)),
                    ("proposalTtlMinutes", number(o.proposal_ttl_minutes)),
                    ("approvalTtlMinutes", number(o.approval_ttl_minutes)),
                    ("maxPendingProposals", number(o.max_pending_proposals)),
                    (
                        "countTowardWorkerLimit",
                        Value::Bool(o.count_toward_worker_limit),
                    ),
                    (
                        "restartHealthTimeoutSeconds",
                        number(o.restart_health_timeout_seconds),
                    ),
                    (
                        "restartIdleWaitSeconds",
                        number(o.restart_idle_wait_seconds),
                    ),
                    (
                        "sessionGrantMaxMinutes",
                        number(o.session_grant_max_minutes),
                    ),
                    (
                        "fullAutoDefaultMinutes",
                        number(o.full_auto_default_minutes),
                    ),
                    ("fullAutoMaxMinutes", number(o.full_auto_max_minutes)),
                ]),
            ),
            (
                "researcher",
                object(vec![
                    ("configured", Value::Bool(self.researcher.configured)),
                    ("enabled", Value::Bool(self.researcher.enabled)),
                    ("role", text(&self.researcher.role)),
                    ("outputDir", text(&self.researcher.output_dir)),
                    ("userAgent", text(&self.researcher.user_agent)),
                ]),
            ),
            (
                "mcpServers",
                Value::Array(self.mcp_servers.iter().map(mcp_server).collect()),
            ),
            (
                "promptRelay",
                object(vec![
                    ("present", Value::Bool(self.prompt_relay.present)),
                    ("enabled", Value::Bool(self.prompt_relay.enabled)),
                    (
                        "captureTtlSeconds",
                        number(self.prompt_relay.capture_ttl_seconds),
                    ),
                ]),
            ),
            (
                "nexora",
                object(vec![
                    ("track", text(self.nexora.track)),
                    ("defaultAction", text(self.nexora.default_action)),
                ]),
            ),
            (
                "limits",
                object(vec![("maxWorkers", number(self.limits.max_workers))]),
            ),
            (
                "ledger",
                object(vec![(
                    "keepMigrationBackups",
                    number(self.ledger.keep_migration_backups),
                )]),
            ),
            (
                "layout",
                object(vec![
                    ("spawn", text(self.layout.spawn)),
                    ("pmWidthPercent", number(self.layout.pm_width_percent)),
                    ("minPaneColumns", number(self.layout.min_pane_columns)),
                    ("minPaneRows", number(self.layout.min_pane_rows)),
                ]),
            ),
        ];
        if let Some(worktree) = &self.worktree {
            let mut tree = Vec::new();
            if let Some(setup) = &worktree.setup {
                tree.push(("setup", text(setup)));
            }
            tree.push((
                "setupTimeoutSeconds",
                number(worktree.setup_timeout_seconds),
            ));
            if let Some(teardown) = &worktree.teardown {
                tree.push(("teardown", text(teardown)));
                tree.push((
                    "teardownTimeoutSeconds",
                    number(worktree.teardown_timeout_seconds.unwrap_or_default()),
                ));
            }
            members.push(("worktree", object(tree)));
        }
        members.push(("env", object(vec![("pass", strings(&self.env_pass))])));
        members.push(("hosts", Value::Array(self.hosts.iter().map(host).collect())));
        members.push(("roles", Value::Array(self.roles.iter().map(role).collect())));
        members.push(("warnings", strings(&self.warnings)));
        object(members)
    }

    /// `JSON.stringify(roleConfig, null, 2)`.
    pub fn to_json(&self) -> String {
        js::stringify(&self.to_value(), 2).to_utf8_lossy()
    }
}
