CREATE TABLE agent_panes (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  agent_id TEXT NOT NULL,
  workspace_id TEXT,
  pane_id TEXT,
  worktree_path TEXT,
  branch TEXT,
  base_sha TEXT CHECK (base_sha IS NULL OR length(base_sha) = 40),
  generation INTEGER NOT NULL CHECK (generation >= 1),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (project_id, agent_id),
  FOREIGN KEY (project_id, agent_id) REFERENCES agents(project_id, agent_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE fallback_panes (
  project_id TEXT NOT NULL REFERENCES projects(project_id) PRIMARY KEY,
  workspace_id TEXT NOT NULL,
  pane_id TEXT NOT NULL,
  created_at TEXT NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE orphan_panes (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  pane_id TEXT NOT NULL,
  agent_id TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, pane_id),
  FOREIGN KEY (project_id, agent_id) REFERENCES agents(project_id, agent_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE pm_restarts (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  restart_id TEXT NOT NULL,
  agent_id TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  summarized_generation INTEGER NOT NULL CHECK (summarized_generation >= 1),
  summary_json TEXT NOT NULL,
  summary_hash TEXT NOT NULL CHECK (length(summary_hash) = 64),
  consumed INTEGER NOT NULL DEFAULT 0 CHECK (consumed IN (0, 1)),
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, restart_id),
  UNIQUE (project_id, agent_id, sequence),
  FOREIGN KEY (project_id, agent_id) REFERENCES agents(project_id, agent_id)
) STRICT, WITHOUT ROWID;

CREATE TRIGGER immutable_pm_restarts_content BEFORE UPDATE OF restart_id, agent_id, sequence, summarized_generation, summary_json, summary_hash, created_at ON pm_restarts BEGIN SELECT RAISE(ABORT, 'pm restart summaries are immutable'); END;
CREATE TRIGGER immutable_pm_restarts_delete BEFORE DELETE ON pm_restarts BEGIN SELECT RAISE(ABORT, 'pm restart summaries are immutable'); END;
