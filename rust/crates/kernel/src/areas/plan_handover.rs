//! The texts of a plan reassignment: the handover the new owner receives and the notice the developers holding a package
//! of the plan receive. Pure functions over facts the reassignment gathered.

/// One live package of the plan at the time of the handover.
#[derive(Clone, Debug, PartialEq)]
pub struct HandoverPackage {
    pub package_id: String,
    pub assignee_agent_id: Option<String>,
    pub progress: String,
}

/// What the new owner needs to carry on: the plan, its live packages and the work still pending.
#[derive(Clone, Debug, PartialEq)]
pub struct HandoverFacts {
    pub plan_id: String,
    pub state: String,
    pub current_revision: i64,
    pub approved_revision: Option<i64>,
    pub from_agent_id: String,
    pub reason: Option<String>,
    pub packages: Vec<HandoverPackage>,
    pub pending_reports: Vec<String>,
    pub pending_reviews: Vec<String>,
    pub pending_integrations: Vec<String>,
}

fn list(ids: &[String]) -> String {
    if ids.is_empty() {
        "none".to_string()
    } else {
        ids.join(", ")
    }
}

/// The `plan_handover` message to the new owner.
pub fn handover_body(facts: &HandoverFacts) -> String {
    let mut lines = vec![format!(
        "You now own plan {} (reassigned from {}{}).",
        facts.plan_id,
        facts.from_agent_id,
        facts
            .reason
            .as_ref()
            .map(|reason| format!(": {reason}"))
            .unwrap_or_default()
    )];
    let approved = facts
        .approved_revision
        .map_or("none".to_string(), |revision| revision.to_string());
    lines.push(format!(
        "State: {}; current revision {}; approved revision {approved}.",
        facts.state, facts.current_revision
    ));
    if facts.packages.is_empty() {
        lines.push("Live packages: none.".to_string());
    } else {
        lines.push("Live packages:".to_string());
        for package in &facts.packages {
            lines.push(format!(
                "- {}: {}, assignee {}",
                package.package_id,
                package.progress,
                package.assignee_agent_id.as_deref().unwrap_or("none")
            ));
        }
    }
    lines.push("Pending:".to_string());
    lines.push(format!(
        "- package reports without a passed review: {}",
        list(&facts.pending_reports)
    ));
    lines.push(format!(
        "- reviews requested or running: {}",
        list(&facts.pending_reviews)
    ));
    lines.push(format!(
        "- integrations not yet signed off: {}",
        list(&facts.pending_integrations)
    ));
    lines.push(format!(
        "Read the plan with cstan plan show {} and carry on from its state.",
        facts.plan_id
    ));
    lines.join("\n")
}

/// The `architect_changed` message to a developer holding the given packages of the plan.
pub fn architect_changed_body(plan_id: &str, package_ids: &[String], new_owner: &str) -> String {
    let refs: Vec<String> = package_ids
        .iter()
        .map(|package| format!("{plan_id}/{package}"))
        .collect();
    format!(
        "The architect of plan {plan_id} changed: questions about {} now go to {new_owner}: cstan send {new_owner} \"<question>\"",
        refs.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> HandoverFacts {
        HandoverFacts {
            plan_id: "plan-3".into(),
            state: "approved".into(),
            current_revision: 2,
            approved_revision: Some(2),
            from_agent_id: "architect-1".into(),
            reason: Some("the pane was lost".into()),
            packages: vec![
                HandoverPackage {
                    package_id: "kernel".into(),
                    assignee_agent_id: Some("dev-4".into()),
                    progress: "reported".into(),
                },
                HandoverPackage {
                    package_id: "docs".into(),
                    assignee_agent_id: None,
                    progress: "unassigned".into(),
                },
            ],
            pending_reports: vec!["report-9".into()],
            pending_reviews: vec![],
            pending_integrations: vec!["integration-2".into(), "integration-3".into()],
        }
    }

    #[test]
    fn the_handover_names_the_plan_packages_and_pending_ids() {
        let body = handover_body(&facts());
        assert!(body.starts_with(
            "You now own plan plan-3 (reassigned from architect-1: the pane was lost)."
        ));
        assert!(body.contains("State: approved; current revision 2; approved revision 2."));
        assert!(body.contains("- kernel: reported, assignee dev-4"));
        assert!(body.contains("- docs: unassigned, assignee none"));
        assert!(body.contains("package reports without a passed review: report-9"));
        assert!(body.contains("reviews requested or running: none"));
        assert!(body.contains("integrations not yet signed off: integration-2, integration-3"));
    }

    #[test]
    fn a_handover_without_reason_packages_or_approval_is_plain() {
        let mut bare = facts();
        bare.reason = None;
        bare.approved_revision = None;
        bare.packages.clear();
        let body = handover_body(&bare);
        assert!(body.starts_with("You now own plan plan-3 (reassigned from architect-1)."));
        assert!(body.contains("approved revision none."));
        assert!(body.contains("Live packages: none."));
    }

    #[test]
    fn the_architect_changed_notice_gives_the_new_address_for_each_package() {
        let body =
            architect_changed_body("plan-3", &["kernel".into(), "docs".into()], "architect-2");
        assert_eq!(
            body,
            "The architect of plan plan-3 changed: questions about plan-3/kernel, plan-3/docs now go to architect-2: cstan send architect-2 \"<question>\""
        );
    }
}
