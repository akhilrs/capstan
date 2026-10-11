-- Branch cleanup records: an offer (or an automatic run) is a cleanups row, each branch it considered a cleanup_branches row.
-- Rows are never deleted; a cleanup state and a branch outcome only move forward.
CREATE TABLE cleanups (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  cleanup_id TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  trigger TEXT NOT NULL CHECK (trigger IN ('confirm', 'manual')),
  integration_id TEXT,
  requested_by TEXT NOT NULL,
  mode TEXT NOT NULL CHECK (mode IN ('ask', 'auto')),
  state TEXT NOT NULL CHECK (state IN ('offered', 'applied', 'declined', 'superseded')),
  message_id TEXT,
  created_at TEXT NOT NULL,
  completed_at TEXT,
  PRIMARY KEY (project_id, cleanup_id),
  UNIQUE (project_id, sequence),
  FOREIGN KEY (project_id, integration_id) REFERENCES integrations(project_id, integration_id),
  CHECK ((state = 'offered' AND completed_at IS NULL) OR (state <> 'offered' AND completed_at IS NOT NULL))
) STRICT, WITHOUT ROWID;

CREATE TABLE cleanup_branches (
  project_id TEXT NOT NULL,
  cleanup_id TEXT NOT NULL,
  branch TEXT NOT NULL CHECK (length(branch) > 0),
  sha TEXT NOT NULL CHECK (length(sha) = 40 AND sha NOT GLOB '*[^0-9a-f]*'),
  class TEXT NOT NULL CHECK (class IN ('landed', 'landed_by_content', 'unlanded')),
  report_id TEXT,
  integration_id TEXT,
  worktree_path TEXT,
  outcome TEXT NOT NULL CHECK (outcome IN ('offered', 'deleted', 'skipped')),
  reason TEXT,
  decided_at TEXT,
  PRIMARY KEY (project_id, cleanup_id, branch),
  FOREIGN KEY (project_id, cleanup_id) REFERENCES cleanups(project_id, cleanup_id),
  CHECK ((outcome = 'offered' AND decided_at IS NULL) OR (outcome <> 'offered' AND decided_at IS NOT NULL))
) STRICT, WITHOUT ROWID;

CREATE TRIGGER immutable_cleanups_identity BEFORE UPDATE OF project_id, cleanup_id, sequence, trigger, integration_id, requested_by, mode, created_at ON cleanups BEGIN SELECT RAISE(ABORT, 'cleanup identity is immutable'); END;
CREATE TRIGGER cleanups_move_forward BEFORE UPDATE OF state ON cleanups
  WHEN NOT (OLD.state = 'offered' AND NEW.state IN ('applied', 'declined', 'superseded'))
  BEGIN SELECT RAISE(ABORT, 'a cleanup state only moves forward'); END;
CREATE TRIGGER cleanups_message_once BEFORE UPDATE OF message_id ON cleanups WHEN OLD.message_id IS NOT NULL BEGIN SELECT RAISE(ABORT, 'a cleanup is announced once'); END;
CREATE TRIGGER immutable_cleanups_delete BEFORE DELETE ON cleanups BEGIN SELECT RAISE(ABORT, 'cleanups are immutable'); END;
CREATE TRIGGER immutable_cleanup_branches_identity BEFORE UPDATE OF project_id, cleanup_id, branch, sha, class, report_id, integration_id, worktree_path ON cleanup_branches BEGIN SELECT RAISE(ABORT, 'cleanup branch identity is immutable'); END;
CREATE TRIGGER cleanup_branches_move_forward BEFORE UPDATE OF outcome ON cleanup_branches
  WHEN NOT (OLD.outcome = 'offered' AND NEW.outcome IN ('deleted', 'skipped'))
  BEGIN SELECT RAISE(ABORT, 'a cleanup branch outcome only moves forward'); END;
CREATE TRIGGER immutable_cleanup_branches_delete BEFORE DELETE ON cleanup_branches BEGIN SELECT RAISE(ABORT, 'cleanup branches are immutable'); END;
CREATE INDEX cleanups_by_state ON cleanups(project_id, state);
