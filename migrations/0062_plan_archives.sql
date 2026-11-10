-- Plan archives: the operator's statement that a plan is no longer followed (retired, abandoned or replaced outside the
-- ledger). The row belongs to the plan and says who archived it and why; it changes no plan state. No trigger: a row is
-- deleted to unarchive.
CREATE TABLE IF NOT EXISTS plan_archives (
  project_id TEXT NOT NULL,
  plan_id TEXT NOT NULL,
  archived_at TEXT NOT NULL,
  archived_by TEXT NOT NULL,
  reason TEXT NOT NULL CHECK (length(reason) BETWEEN 1 AND 500),
  PRIMARY KEY (project_id, plan_id),
  FOREIGN KEY (project_id, plan_id) REFERENCES plans(project_id, plan_id)
) STRICT, WITHOUT ROWID;
