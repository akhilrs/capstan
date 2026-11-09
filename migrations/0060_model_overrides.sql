-- Model overrides (the user's or the PM's choice of model for a plan or a requirement, per role kind) and the model each
-- agent was started with. No existing table is altered. agent_models rows are never deleted: release, replace and end keep
-- them, because a replacement reads the row of the agent it replaces.
CREATE TABLE IF NOT EXISTS model_overrides (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  scope_type TEXT NOT NULL CHECK (scope_type IN ('plan', 'requirement')),
  scope_id TEXT NOT NULL,
  role_kind TEXT NOT NULL DEFAULT '' CHECK (role_kind IN ('', 'Developer', 'Verifier', 'Supervisor')),
  model TEXT NOT NULL CHECK (length(model) BETWEEN 1 AND 100),
  set_by TEXT NOT NULL,
  set_at TEXT NOT NULL,
  PRIMARY KEY (project_id, scope_type, scope_id, role_kind)
) STRICT, WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS agent_models (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  agent_id TEXT NOT NULL,
  model TEXT NOT NULL CHECK (length(model) BETWEEN 1 AND 100),
  source TEXT NOT NULL CHECK (source IN ('spawn', 'task', 'plan', 'role', 'host', 'defaults')),
  scope_id TEXT,
  PRIMARY KEY (project_id, agent_id),
  FOREIGN KEY (project_id, agent_id) REFERENCES agents(project_id, agent_id)
) STRICT, WITHOUT ROWID;
