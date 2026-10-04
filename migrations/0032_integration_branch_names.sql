-- Integration branches are named integration/<plan-id>-<slug>, so the branch no longer equals
-- 'capstan/integration/' || integration_id. Every row, the UNIQUE (project_id, branch), the index and the
-- triggers of 0019 are kept.
-- Dropping the table deletes the parents of integration_reports, plan_signoffs, reviews and
-- integration_covered_reports, so their foreign keys are checked at commit, by when the rows are back.
PRAGMA defer_foreign_keys = ON;

CREATE TEMP TABLE integrations_keep AS SELECT * FROM integrations;

DROP TABLE integrations;

CREATE TABLE integrations (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  integration_id TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  base_sha TEXT NOT NULL CHECK (length(base_sha) = 40 AND base_sha NOT GLOB '*[^0-9a-f]*'),
  branch TEXT NOT NULL,
  requested_by TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('running', 'merged', 'conflicted', 'failed', 'confirmed', 'discarded')),
  head_sha TEXT CHECK (head_sha IS NULL OR (length(head_sha) = 40 AND head_sha NOT GLOB '*[^0-9a-f]*')),
  conflict_report_id TEXT,
  conflict_files_json TEXT,
  conflict_files_omitted INTEGER,
  failure_reason TEXT,
  created_at TEXT NOT NULL,
  completed_at TEXT,
  PRIMARY KEY (project_id, integration_id),
  UNIQUE (project_id, sequence),
  UNIQUE (project_id, branch),
  CHECK (head_sha IS NULL OR head_sha <> base_sha),
  CHECK (conflict_files_json IS NULL OR (json_valid(conflict_files_json) AND json_array_length(conflict_files_json) > 0)),
  CHECK ((state IN ('merged', 'confirmed', 'discarded') AND head_sha IS NOT NULL) OR (state NOT IN ('merged', 'confirmed', 'discarded') AND head_sha IS NULL)),
  CHECK ((state = 'conflicted' AND conflict_report_id IS NOT NULL AND conflict_files_json IS NOT NULL AND conflict_files_omitted IS NOT NULL AND conflict_files_omitted >= 0) OR (state <> 'conflicted' AND conflict_report_id IS NULL AND conflict_files_json IS NULL AND conflict_files_omitted IS NULL)),
  CHECK ((state = 'failed' AND failure_reason IS NOT NULL) OR (state <> 'failed' AND failure_reason IS NULL)),
  CHECK ((state = 'running' AND completed_at IS NULL) OR (state <> 'running' AND completed_at IS NOT NULL))
) STRICT, WITHOUT ROWID;

INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, head_sha, conflict_report_id,
    conflict_files_json, conflict_files_omitted, failure_reason, created_at, completed_at)
SELECT project_id, integration_id, sequence, base_sha, branch, requested_by, state, head_sha, conflict_report_id,
    conflict_files_json, conflict_files_omitted, failure_reason, created_at, completed_at
FROM integrations_keep ORDER BY sequence;

DROP TABLE integrations_keep;

CREATE UNIQUE INDEX one_running_integration ON integrations(project_id) WHERE state = 'running';

CREATE TRIGGER integrations_conflict_report_is_a_member BEFORE UPDATE OF conflict_report_id ON integrations
  WHEN NEW.conflict_report_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM integration_reports WHERE project_id = NEW.project_id AND integration_id = NEW.integration_id AND report_id = NEW.conflict_report_id)
  BEGIN SELECT RAISE(ABORT, 'the conflicting report is not part of the integration'); END;
CREATE TRIGGER immutable_integrations_identity BEFORE UPDATE OF project_id, integration_id, sequence, base_sha, branch, requested_by, created_at ON integrations BEGIN SELECT RAISE(ABORT, 'integration identity is immutable'); END;
CREATE TRIGGER integrations_move_forward BEFORE UPDATE OF state ON integrations
  WHEN NOT ((OLD.state = 'running' AND NEW.state IN ('merged', 'conflicted', 'failed')) OR (OLD.state = 'merged' AND NEW.state IN ('confirmed', 'discarded')))
  BEGIN SELECT RAISE(ABORT, 'an integration state only moves forward'); END;
CREATE TRIGGER immutable_integrations_delete BEFORE DELETE ON integrations BEGIN SELECT RAISE(ABORT, 'integrations are immutable'); END;
