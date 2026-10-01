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
  failure_reason TEXT,
  created_at TEXT NOT NULL,
  completed_at TEXT,
  PRIMARY KEY (project_id, integration_id),
  UNIQUE (project_id, sequence),
  UNIQUE (project_id, branch),
  CHECK ((state IN ('merged', 'confirmed', 'discarded') AND head_sha IS NOT NULL) OR (state NOT IN ('merged', 'confirmed', 'discarded') AND head_sha IS NULL)),
  CHECK ((state = 'conflicted' AND conflict_report_id IS NOT NULL AND conflict_files_json IS NOT NULL) OR (state <> 'conflicted' AND conflict_report_id IS NULL AND conflict_files_json IS NULL)),
  CHECK ((state = 'failed' AND failure_reason IS NOT NULL) OR (state <> 'failed' AND failure_reason IS NULL)),
  CHECK ((state = 'running' AND completed_at IS NULL) OR (state <> 'running' AND completed_at IS NOT NULL))
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX one_running_integration ON integrations(project_id) WHERE state = 'running';

CREATE TABLE integration_reports (
  project_id TEXT NOT NULL,
  integration_id TEXT NOT NULL,
  position INTEGER NOT NULL CHECK (position >= 1),
  report_id TEXT NOT NULL,
  PRIMARY KEY (project_id, integration_id, position),
  UNIQUE (project_id, integration_id, report_id),
  FOREIGN KEY (project_id, integration_id) REFERENCES integrations(project_id, integration_id),
  FOREIGN KEY (project_id, report_id) REFERENCES agent_reports(project_id, report_id)
) STRICT, WITHOUT ROWID;

CREATE TRIGGER immutable_integration_reports_update BEFORE UPDATE ON integration_reports BEGIN SELECT RAISE(ABORT, 'integration reports are immutable'); END;
CREATE TRIGGER immutable_integration_reports_delete BEFORE DELETE ON integration_reports BEGIN SELECT RAISE(ABORT, 'integration reports are immutable'); END;
CREATE TRIGGER immutable_integrations_identity BEFORE UPDATE OF project_id, integration_id, sequence, base_sha, branch, requested_by, created_at ON integrations BEGIN SELECT RAISE(ABORT, 'integration identity is immutable'); END;
CREATE TRIGGER integrations_move_forward BEFORE UPDATE OF state ON integrations
  WHEN NOT ((OLD.state = 'running' AND NEW.state IN ('merged', 'conflicted', 'failed')) OR (OLD.state = 'merged' AND NEW.state IN ('confirmed', 'discarded')))
  BEGIN SELECT RAISE(ABORT, 'an integration state only moves forward'); END;
CREATE TRIGGER immutable_integrations_delete BEFORE DELETE ON integrations BEGIN SELECT RAISE(ABORT, 'integrations are immutable'); END;

CREATE TABLE reviews_next (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  review_id TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  round INTEGER NOT NULL CHECK (round >= 1),
  subject_report_id TEXT,
  subject_integration_id TEXT,
  commit_sha TEXT NOT NULL CHECK (length(commit_sha) = 40 AND commit_sha NOT GLOB '*[^0-9a-f]*'),
  base_sha TEXT NOT NULL CHECK (length(base_sha) = 40 AND base_sha NOT GLOB '*[^0-9a-f]*'),
  author_agent_id TEXT,
  author_actor_id TEXT,
  requested_by_actor_id TEXT NOT NULL,
  reviewer_role TEXT NOT NULL,
  reviewer_agent_id TEXT NOT NULL,
  reviewer_actor_id TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('started', 'passed', 'findings', 'failed', 'cancelled')),
  verdict_text TEXT,
  failure_reason TEXT,
  notified_message_id TEXT,
  created_at TEXT NOT NULL,
  completed_at TEXT,
  PRIMARY KEY (project_id, review_id),
  UNIQUE (project_id, sequence),
  UNIQUE (project_id, subject_report_id, round),
  UNIQUE (project_id, subject_integration_id, round),
  FOREIGN KEY (project_id, subject_report_id) REFERENCES agent_reports(project_id, report_id),
  FOREIGN KEY (project_id, subject_integration_id) REFERENCES integrations(project_id, integration_id),
  FOREIGN KEY (project_id, reviewer_agent_id) REFERENCES agents(project_id, agent_id),
  FOREIGN KEY (project_id, reviewer_actor_id) REFERENCES actors(project_id, actor_id),
  CHECK ((subject_report_id IS NOT NULL) <> (subject_integration_id IS NOT NULL)),
  CHECK ((subject_report_id IS NOT NULL AND author_agent_id IS NOT NULL AND author_actor_id IS NOT NULL) OR (subject_integration_id IS NOT NULL AND author_agent_id IS NULL AND author_actor_id IS NULL)),
  CHECK (reviewer_actor_id <> author_actor_id),
  CHECK (reviewer_agent_id <> author_agent_id),
  CHECK ((state IN ('passed', 'findings') AND verdict_text IS NOT NULL) OR (state NOT IN ('passed', 'findings') AND verdict_text IS NULL)),
  CHECK ((state IN ('failed', 'cancelled') AND failure_reason IS NOT NULL) OR (state NOT IN ('failed', 'cancelled') AND failure_reason IS NULL))
) STRICT, WITHOUT ROWID;

INSERT INTO reviews_next(project_id, review_id, sequence, round, subject_report_id, subject_integration_id, commit_sha, base_sha, author_agent_id,
    author_actor_id, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, state, verdict_text, failure_reason,
    notified_message_id, created_at, completed_at)
SELECT project_id, review_id, sequence, round, subject_report_id, NULL, commit_sha, base_sha, author_agent_id,
    author_actor_id, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, state, verdict_text, failure_reason,
    notified_message_id, created_at, completed_at
FROM reviews;

DROP TABLE reviews;
ALTER TABLE reviews_next RENAME TO reviews;

CREATE UNIQUE INDEX one_open_review_per_report ON reviews(project_id, subject_report_id) WHERE state = 'started' AND subject_report_id IS NOT NULL;
CREATE UNIQUE INDEX one_open_review_per_integration ON reviews(project_id, subject_integration_id) WHERE state = 'started' AND subject_integration_id IS NOT NULL;
CREATE INDEX reviews_by_reviewer ON reviews(project_id, reviewer_agent_id, state);

CREATE TRIGGER immutable_reviews_identity BEFORE UPDATE OF project_id, review_id, sequence, round, subject_report_id, subject_integration_id, commit_sha, base_sha, author_agent_id, author_actor_id, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, created_at ON reviews BEGIN SELECT RAISE(ABORT, 'review identity is immutable'); END;
CREATE TRIGGER finished_reviews_are_final BEFORE UPDATE OF state, verdict_text, failure_reason, completed_at ON reviews WHEN OLD.state <> 'started' BEGIN SELECT RAISE(ABORT, 'a finished review is final'); END;
CREATE TRIGGER reviews_notified_once BEFORE UPDATE OF notified_message_id ON reviews WHEN OLD.notified_message_id IS NOT NULL BEGIN SELECT RAISE(ABORT, 'a review is announced once'); END;
CREATE TRIGGER immutable_reviews_delete BEFORE DELETE ON reviews BEGIN SELECT RAISE(ABORT, 'reviews are immutable'); END;
CREATE TRIGGER integration_reviewer_is_no_author BEFORE INSERT ON reviews WHEN NEW.subject_integration_id IS NOT NULL
BEGIN
  SELECT RAISE(ABORT, 'a reviewer cannot be the author of a merged report')
  WHERE EXISTS (
    SELECT 1 FROM integration_reports ir
    JOIN agent_reports r ON r.project_id = ir.project_id AND r.report_id = ir.report_id
    WHERE ir.project_id = NEW.project_id AND ir.integration_id = NEW.subject_integration_id
      AND (r.agent_id = NEW.reviewer_agent_id OR r.actor_id = NEW.reviewer_actor_id));
END;
