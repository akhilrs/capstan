-- Design approvals: the user's decision on the report of a design package (plan_body key design: true). A decision is
-- tied to the report it is about, so a package that depends on the design is assigned only once the latest report is
-- approved. Rows are append-only; the latest row per (project, plan, package) is the current decision.
CREATE TABLE design_approvals (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  decision_id TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  plan_id TEXT NOT NULL,
  package_id TEXT NOT NULL,
  decision TEXT NOT NULL CHECK (decision IN ('approved', 'changes')),
  report_id TEXT NOT NULL,
  link TEXT,
  asked_user TEXT NOT NULL CHECK (length(asked_user) > 0),
  decided_by TEXT NOT NULL,
  decided_at TEXT NOT NULL,
  PRIMARY KEY (project_id, decision_id),
  UNIQUE (project_id, sequence),
  FOREIGN KEY (project_id, plan_id, package_id) REFERENCES plan_packages(project_id, plan_id, package_id),
  FOREIGN KEY (project_id, report_id) REFERENCES agent_reports(project_id, report_id),
  CHECK (decision <> 'approved' OR link IS NOT NULL)
) STRICT, WITHOUT ROWID;

CREATE INDEX design_approvals_by_package ON design_approvals(project_id, plan_id, package_id, sequence);

CREATE TRIGGER immutable_design_approvals_update BEFORE UPDATE ON design_approvals BEGIN SELECT RAISE(ABORT, 'design approvals are immutable'); END;
CREATE TRIGGER immutable_design_approvals_delete BEFORE DELETE ON design_approvals BEGIN SELECT RAISE(ABORT, 'design approvals are immutable'); END;
