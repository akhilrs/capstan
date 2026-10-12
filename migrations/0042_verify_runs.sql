-- Verify runs: the project's configured verification commands run against the commit of one report or integration.
-- A run is queued, claimed (running) and finished once; a finished run is final. The ledger records what ran, at which
-- commit, with which commands and how it ended, so a review request can be gated on it.
CREATE TABLE verify_runs (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  run_id TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  subject_report_id TEXT,
  subject_integration_id TEXT,
  commit_sha TEXT NOT NULL CHECK (length(commit_sha) = 40 AND commit_sha NOT GLOB '*[^0-9a-f]*'),
  commands_json TEXT NOT NULL CHECK (json_valid(commands_json) AND json_type(commands_json) = 'array'),
  commands_sha TEXT NOT NULL CHECK (length(commands_sha) > 0),
  trigger TEXT NOT NULL CHECK (trigger IN ('manual', 'auto')),
  requested_by_actor_id TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('queued', 'running', 'passed', 'failed', 'timeout', 'error', 'interrupted', 'cancelled')),
  queued_at TEXT NOT NULL,
  started_at TEXT,
  finished_at TEXT,
  exit_code INTEGER,
  failed_step INTEGER CHECK (failed_step IS NULL OR failed_step >= 1),
  duration_ms INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
  output_tail TEXT NOT NULL DEFAULT '' CHECK (length(CAST(output_tail AS BLOB)) <= 12288),
  output_truncated INTEGER NOT NULL DEFAULT 0 CHECK (output_truncated IN (0, 1)),
  log_path TEXT,
  scratch_path TEXT,
  pgid INTEGER CHECK (pgid IS NULL OR pgid > 1),
  leader_start TEXT,
  orphan_cleared_at TEXT,
  notified_message_id TEXT,
  PRIMARY KEY (project_id, run_id),
  UNIQUE (project_id, sequence),
  FOREIGN KEY (project_id, subject_report_id) REFERENCES agent_reports(project_id, report_id),
  FOREIGN KEY (project_id, subject_integration_id) REFERENCES integrations(project_id, integration_id),
  CHECK ((subject_report_id IS NOT NULL) + (subject_integration_id IS NOT NULL) = 1),
  CHECK ((state = 'queued' AND started_at IS NULL AND finished_at IS NULL)
      OR (state = 'running' AND started_at IS NOT NULL AND finished_at IS NULL)
      OR (state = 'cancelled' AND finished_at IS NOT NULL)
      OR (state NOT IN ('queued', 'running', 'cancelled') AND started_at IS NOT NULL AND finished_at IS NOT NULL))
) STRICT, WITHOUT ROWID;

CREATE INDEX verify_runs_by_commit ON verify_runs(project_id, commit_sha, commands_sha);
CREATE INDEX verify_runs_by_state ON verify_runs(project_id, state, sequence);
CREATE UNIQUE INDEX one_active_verify_run ON verify_runs(project_id, commit_sha, commands_sha) WHERE state IN ('queued', 'running');

CREATE TRIGGER immutable_verify_runs_identity BEFORE UPDATE OF project_id, run_id, sequence, subject_report_id, subject_integration_id, commit_sha, commands_json, commands_sha, trigger, requested_by_actor_id, queued_at ON verify_runs
  BEGIN SELECT RAISE(ABORT, 'verify run identity is immutable'); END;
CREATE TRIGGER verify_runs_move_forward BEFORE UPDATE OF state ON verify_runs
  WHEN NEW.state IS NOT OLD.state AND NOT (
    (OLD.state = 'queued' AND NEW.state IN ('running', 'cancelled'))
    OR (OLD.state = 'running' AND NEW.state IN ('passed', 'failed', 'timeout', 'error', 'interrupted')))
  BEGIN SELECT RAISE(ABORT, 'a verify run state only moves forward'); END;
CREATE TRIGGER finished_verify_runs_are_final BEFORE UPDATE OF state, started_at, finished_at, exit_code, failed_step, duration_ms, output_tail, output_truncated ON verify_runs
  WHEN OLD.state NOT IN ('queued', 'running')
  BEGIN SELECT RAISE(ABORT, 'a finished verify run is final'); END;
CREATE TRIGGER verify_runs_passed_needs_clean_exit BEFORE UPDATE OF state ON verify_runs
  WHEN NEW.state = 'passed' AND NOT (OLD.state = 'running' AND NEW.exit_code = 0 AND NEW.finished_at IS NOT NULL)
  BEGIN SELECT RAISE(ABORT, 'a verify run passes only from running with exit code 0'); END;
CREATE TRIGGER verify_runs_process_is_final BEFORE UPDATE OF pgid, leader_start ON verify_runs
  WHEN (OLD.pgid IS NOT NULL AND NEW.pgid IS NOT OLD.pgid) OR (OLD.leader_start IS NOT NULL AND NEW.leader_start IS NOT OLD.leader_start)
  BEGIN SELECT RAISE(ABORT, 'the recorded verify process is final'); END;
CREATE TRIGGER verify_runs_notified_once BEFORE UPDATE OF notified_message_id ON verify_runs WHEN OLD.notified_message_id IS NOT NULL
  BEGIN SELECT RAISE(ABORT, 'a verify run is announced once'); END;
CREATE TRIGGER immutable_verify_runs_delete BEFORE DELETE ON verify_runs BEGIN SELECT RAISE(ABORT, 'verify runs are immutable'); END;
