-- Workspace repos: a project may span several repositories. An agent holds one worktree per repo, a report may name one
-- commit per repo, a verify run belongs to one repo, and one integration per repo is gathered into an integration set.
-- No table is rebuilt. '' is the one repo of a project without [repos]: the root.

-- The worktrees an agent holds, per generation and repo. A removed row is revived when the worktree is added again.
CREATE TABLE IF NOT EXISTS agent_worktrees (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  agent_id TEXT NOT NULL,
  generation INTEGER NOT NULL CHECK (generation >= 1),
  repo TEXT NOT NULL,
  worktree_path TEXT NOT NULL CHECK (length(worktree_path) > 0),
  branch TEXT NOT NULL CHECK (length(branch) > 0),
  base_sha TEXT NOT NULL CHECK (length(base_sha) = 40 AND base_sha NOT GLOB '*[^0-9a-f]*'),
  created_at TEXT NOT NULL,
  removed_at TEXT,
  PRIMARY KEY (project_id, agent_id, generation, repo),
  FOREIGN KEY (project_id, agent_id) REFERENCES agents(project_id, agent_id)
) STRICT, WITHOUT ROWID;

-- The commits of one report, one per repo; the first is the report's own commit.
CREATE TABLE IF NOT EXISTS report_commits (
  project_id TEXT NOT NULL,
  report_id TEXT NOT NULL,
  repo TEXT NOT NULL,
  commit_sha TEXT NOT NULL CHECK (length(commit_sha) = 40 AND commit_sha NOT GLOB '*[^0-9a-f]*'),
  branch TEXT NOT NULL CHECK (length(branch) > 0),
  base_sha TEXT NOT NULL CHECK (length(base_sha) = 40 AND base_sha NOT GLOB '*[^0-9a-f]*'),
  PRIMARY KEY (project_id, report_id, repo),
  FOREIGN KEY (project_id, report_id) REFERENCES agent_reports(project_id, report_id)
) STRICT, WITHOUT ROWID;

CREATE TRIGGER IF NOT EXISTS immutable_report_commits_update BEFORE UPDATE ON report_commits BEGIN SELECT RAISE(ABORT, 'report commits are immutable'); END;
CREATE TRIGGER IF NOT EXISTS immutable_report_commits_delete BEFORE DELETE ON report_commits BEGIN SELECT RAISE(ABORT, 'report commits are immutable'); END;

-- An integration set: one integration per repo, merged together. The states and their moves are kept by the kernel.
CREATE TABLE IF NOT EXISTS integration_sets (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  set_id TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  branch TEXT NOT NULL CHECK (length(branch) > 0),
  state TEXT NOT NULL,
  reason TEXT,
  reset_at TEXT,
  created_by TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, set_id),
  UNIQUE (project_id, sequence)
) STRICT, WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS integration_set_members (
  project_id TEXT NOT NULL,
  set_id TEXT NOT NULL,
  repo TEXT NOT NULL,
  position INTEGER NOT NULL CHECK (position >= 0),
  integration_id TEXT,
  attempt INTEGER NOT NULL DEFAULT 0 CHECK (attempt >= 0),
  PRIMARY KEY (project_id, set_id, repo),
  FOREIGN KEY (project_id, set_id) REFERENCES integration_sets(project_id, set_id),
  FOREIGN KEY (project_id, integration_id) REFERENCES integrations(project_id, integration_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS integration_set_members_by_integration ON integration_set_members(project_id, integration_id) WHERE integration_id IS NOT NULL;

-- The repo a cleanup ran in. A cleanup without a row reads as the root repo ''.
CREATE TABLE IF NOT EXISTS cleanup_repos (
  project_id TEXT NOT NULL,
  cleanup_id TEXT NOT NULL,
  repo TEXT NOT NULL,
  PRIMARY KEY (project_id, cleanup_id),
  FOREIGN KEY (project_id, cleanup_id) REFERENCES cleanups(project_id, cleanup_id)
) STRICT, WITHOUT ROWID;

-- A verify run belongs to one repo; the old runs belong to the root. Two repos may queue one commit and commands.
ALTER TABLE verify_runs ADD COLUMN repo TEXT NOT NULL DEFAULT ('');

DROP INDEX IF EXISTS one_active_verify_run;
CREATE UNIQUE INDEX IF NOT EXISTS one_active_verify_run ON verify_runs(project_id, repo, commit_sha, commands_sha) WHERE state IN ('queued', 'running');

DROP TRIGGER IF EXISTS immutable_verify_runs_identity;
CREATE TRIGGER IF NOT EXISTS immutable_verify_runs_identity BEFORE UPDATE OF project_id, run_id, sequence, subject_report_id, subject_integration_id, repo, commit_sha, commands_json, commands_sha, trigger, requested_by_actor_id, queued_at ON verify_runs
  BEGIN SELECT RAISE(ABORT, 'verify run identity is immutable'); END;
