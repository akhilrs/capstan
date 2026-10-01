CREATE TABLE reviews (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  review_id TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  round INTEGER NOT NULL CHECK (round >= 1),
  subject_report_id TEXT NOT NULL,
  commit_sha TEXT NOT NULL CHECK (length(commit_sha) = 40 AND commit_sha NOT GLOB '*[^0-9a-f]*'),
  base_sha TEXT NOT NULL CHECK (length(base_sha) = 40 AND base_sha NOT GLOB '*[^0-9a-f]*'),
  author_agent_id TEXT NOT NULL,
  author_actor_id TEXT NOT NULL,
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
  FOREIGN KEY (project_id, subject_report_id) REFERENCES agent_reports(project_id, report_id),
  FOREIGN KEY (project_id, reviewer_agent_id) REFERENCES agents(project_id, agent_id),
  FOREIGN KEY (project_id, reviewer_actor_id) REFERENCES actors(project_id, actor_id),
  CHECK (reviewer_actor_id <> author_actor_id),
  CHECK (reviewer_agent_id <> author_agent_id),
  CHECK ((state IN ('passed', 'findings') AND verdict_text IS NOT NULL) OR (state NOT IN ('passed', 'findings') AND verdict_text IS NULL)),
  CHECK ((state IN ('failed', 'cancelled') AND failure_reason IS NOT NULL) OR (state NOT IN ('failed', 'cancelled') AND failure_reason IS NULL))
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX one_open_review_per_report ON reviews(project_id, subject_report_id) WHERE state = 'started';
CREATE INDEX reviews_by_reviewer ON reviews(project_id, reviewer_agent_id, state);

CREATE TRIGGER immutable_reviews_identity BEFORE UPDATE OF project_id, review_id, sequence, round, subject_report_id, commit_sha, base_sha, author_agent_id, author_actor_id, requested_by_actor_id, reviewer_role, reviewer_agent_id, reviewer_actor_id, created_at ON reviews BEGIN SELECT RAISE(ABORT, 'review identity is immutable'); END;
CREATE TRIGGER finished_reviews_are_final BEFORE UPDATE OF state, verdict_text, failure_reason, completed_at ON reviews WHEN OLD.state <> 'started' BEGIN SELECT RAISE(ABORT, 'a finished review is final'); END;
CREATE TRIGGER reviews_notified_once BEFORE UPDATE OF notified_message_id ON reviews WHEN OLD.notified_message_id IS NOT NULL BEGIN SELECT RAISE(ABORT, 'a review is announced once'); END;
CREATE TRIGGER immutable_reviews_delete BEFORE DELETE ON reviews BEGIN SELECT RAISE(ABORT, 'reviews are immutable'); END;

INSERT INTO role_capabilities(role, capability) VALUES
  ('PM', 'review:request'), ('Verifier', 'review:submit');

INSERT OR IGNORE INTO capability_grants(project_id, actor_id, capability, granted_by, granted_at)
SELECT a.project_id, a.actor_id, rc.capability, internal.actor_id, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM actors a
JOIN role_capabilities rc ON rc.role = a.role AND rc.capability IN ('review:request', 'review:submit')
JOIN actors internal ON internal.project_id = a.project_id AND internal.is_internal = 1
  AND internal.role = 'controller' AND internal.active = 1 AND internal.revoked_at IS NULL
WHERE a.active = 1 AND a.revoked_at IS NULL AND a.role IN ('PM', 'Verifier');
