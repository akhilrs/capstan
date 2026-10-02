CREATE TABLE reviews_next (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  review_id TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  round INTEGER NOT NULL CHECK (round >= 1),
  subject_report_id TEXT,
  subject_integration_id TEXT,
  subject_plan_id TEXT,
  subject_plan_revision INTEGER,
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
  UNIQUE (project_id, subject_plan_id, round),
  FOREIGN KEY (project_id, subject_report_id) REFERENCES agent_reports(project_id, report_id),
  FOREIGN KEY (project_id, subject_integration_id) REFERENCES integrations(project_id, integration_id),
  FOREIGN KEY (project_id, subject_plan_id, subject_plan_revision) REFERENCES plan_revisions(project_id, plan_id, revision),
  FOREIGN KEY (project_id, reviewer_agent_id) REFERENCES agents(project_id, agent_id),
  FOREIGN KEY (project_id, reviewer_actor_id) REFERENCES actors(project_id, actor_id),
  CHECK ((subject_report_id IS NOT NULL) + (subject_integration_id IS NOT NULL) + (subject_plan_id IS NOT NULL) = 1),
  CHECK ((subject_plan_id IS NULL) = (subject_plan_revision IS NULL)),
  CHECK ((subject_integration_id IS NULL AND author_agent_id IS NOT NULL AND author_actor_id IS NOT NULL) OR (subject_integration_id IS NOT NULL AND author_agent_id IS NULL AND author_actor_id IS NULL)),
  CHECK (reviewer_actor_id <> author_actor_id),
  CHECK (reviewer_agent_id <> author_agent_id),
  CHECK ((state IN ('passed', 'findings') AND verdict_text IS NOT NULL) OR (state NOT IN ('passed', 'findings') AND verdict_text IS NULL)),
  CHECK ((state IN ('failed', 'cancelled') AND failure_reason IS NOT NULL) OR (state NOT IN ('failed', 'cancelled') AND failure_reason IS NULL))
) STRICT, WITHOUT ROWID;

INSERT INTO reviews_next(project_id, review_id, sequence, round, subject_report_id, subject_integration_id, commit_sha, base_sha, author_agent_id,
    author_actor_id, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, state, verdict_text, failure_reason,
    notified_message_id, created_at, completed_at)
SELECT project_id, review_id, sequence, round, subject_report_id, subject_integration_id, commit_sha, base_sha, author_agent_id,
    author_actor_id, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, state, verdict_text, failure_reason,
    notified_message_id, created_at, completed_at
FROM reviews;

DROP TABLE reviews;
ALTER TABLE reviews_next RENAME TO reviews;

CREATE UNIQUE INDEX one_open_review_per_report ON reviews(project_id, subject_report_id) WHERE state = 'started' AND subject_report_id IS NOT NULL;
CREATE UNIQUE INDEX one_open_review_per_integration ON reviews(project_id, subject_integration_id) WHERE state = 'started' AND subject_integration_id IS NOT NULL;
CREATE UNIQUE INDEX one_open_review_per_plan ON reviews(project_id, subject_plan_id) WHERE state = 'started' AND subject_plan_id IS NOT NULL;
CREATE INDEX reviews_by_reviewer ON reviews(project_id, reviewer_agent_id, state);

CREATE TRIGGER immutable_reviews_identity BEFORE UPDATE OF project_id, review_id, sequence, round, subject_report_id, subject_integration_id, subject_plan_id, subject_plan_revision, commit_sha, base_sha, author_agent_id, author_actor_id, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, created_at ON reviews BEGIN SELECT RAISE(ABORT, 'review identity is immutable'); END;
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
CREATE TRIGGER plan_review_needs_a_plan_in_review BEFORE INSERT ON reviews WHEN NEW.subject_plan_id IS NOT NULL
BEGIN
  SELECT RAISE(ABORT, 'a plan review needs the current revision of a plan in review')
  WHERE NOT EXISTS (
    SELECT 1 FROM plans WHERE project_id = NEW.project_id AND plan_id = NEW.subject_plan_id
      AND state = 'in_review' AND current_revision = NEW.subject_plan_revision);
END;
