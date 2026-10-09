//! The stall suppression the CPU activity of tool processes feeds (`suppressActiveStalls` of
//! src/controller/messaging.ts). The activity tracker itself is `capstan_herdr::process_activity::ProcessActivityTracker`.

use capstan_herdr::api::ProcessSample;
use capstan_herdr::process_activity::ProcEntry;
use serde_json::Value;
use std::collections::HashMap;

/// A probe's sample as the tracker takes it: milliseconds of CPU, and the command name for the start key (the narrow
/// view of the probe carries no start time).
pub fn proc_entries(sample: &ProcessSample) -> Vec<ProcEntry> {
    sample
        .processes
        .iter()
        .map(|p| ProcEntry {
            pid: p.pid,
            ppid: p.ppid,
            comm: p.comm.clone(),
            cpu_ms: (p.cpu_seconds * 1000.0).round(),
            start_key: p.comm.clone(),
        })
        .collect()
}

/// `suppressActiveStalls`: an agent whose tool processes used CPU within the stall time is not stalled. Returns the
/// evaluation with the stalled list and the stalled attention filtered; blocked attention, transitions and actions are
/// unchanged.
pub fn suppress_active_stalls(
    evaluation: &Value,
    last_child_activity: &HashMap<String, u64>,
    now_ms: i64,
    stall_after_seconds: f64,
) -> Value {
    let active = |agent_id: &str| {
        last_child_activity
            .get(agent_id)
            .is_some_and(|last| ((now_ms - *last as i64) as f64) < stall_after_seconds * 1000.0)
    };
    let mut out = evaluation.clone();
    if let Some(stalled) = evaluation["stalledAgentIds"].as_array() {
        out["stalledAgentIds"] = Value::Array(
            stalled
                .iter()
                .filter(|id| !id.as_str().is_some_and(active))
                .cloned()
                .collect(),
        );
    }
    if let Some(attention) = evaluation["attention"].as_array() {
        out["attention"] = Value::Array(
            attention
                .iter()
                .filter(|item| {
                    !(item["kind"] == "stalled" && item["agentId"].as_str().is_some_and(active))
                })
                .cloned()
                .collect(),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use capstan_herdr::api::ProcessEntry;
    use serde_json::json;

    #[test]
    fn a_sample_becomes_tracker_entries_in_milliseconds() {
        let sample = ProcessSample {
            shell_pid: Some(1),
            processes: vec![ProcessEntry {
                pid: 7,
                ppid: 1,
                comm: "cargo".into(),
                cpu_seconds: 2.5,
            }],
        };
        let entries = proc_entries(&sample);
        assert_eq!(entries[0].cpu_ms, 2500.0);
        assert_eq!(entries[0].start_key, "cargo");
    }

    #[test]
    fn recent_child_activity_suppresses_a_stall() {
        let evaluation = json!({
            "stalledAgentIds": ["a", "b"],
            "attention": [{"kind": "stalled", "agentId": "a"}, {"kind": "blocked", "agentId": "a"}],
            "actions": [],
        });
        let activity = HashMap::from([("a".to_string(), 9_000u64)]);
        let out = suppress_active_stalls(&evaluation, &activity, 10_000, 5.0);
        assert_eq!(out["stalledAgentIds"], json!(["b"]));
        assert_eq!(
            out["attention"],
            json!([{"kind": "blocked", "agentId": "a"}])
        );
    }
}
