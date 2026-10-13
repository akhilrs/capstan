-- Warm reviewers: the Verifier that keeps reviewing the reports and integrations of one plan, so it reads the plan's code once.
-- A plan has at most one unreleased warm reviewer and an agent is warm for at most one plan. A released row is final: it
-- records how many reviews the reviewer gave and why it was released.
CREATE TABLE warm_reviewers (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  plan_id TEXT NOT NULL,
  reviewer_role TEXT NOT NULL,
  agent_id TEXT NOT NULL,
  reviews_count INTEGER NOT NULL CHECK (reviews_count >= 0),
  created_at TEXT NOT NULL,
  last_used_at TEXT NOT NULL,
  released_at TEXT,
  release_reason TEXT CHECK (release_reason IS NULL OR release_reason IN ('signoff', 'cancel', 'idle', 'max_reviews', 'evicted', 'retarget_failed', 'lost')),
  PRIMARY KEY (project_id, plan_id, agent_id),
  FOREIGN KEY (project_id, plan_id) REFERENCES plans(project_id, plan_id),
  FOREIGN KEY (project_id, agent_id) REFERENCES agents(project_id, agent_id),
  CHECK ((released_at IS NULL) = (release_reason IS NULL))
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX one_warm_reviewer_per_plan ON warm_reviewers(project_id, plan_id) WHERE released_at IS NULL;
CREATE UNIQUE INDEX one_warm_plan_per_reviewer ON warm_reviewers(project_id, agent_id) WHERE released_at IS NULL;

CREATE TRIGGER released_warm_reviewers_are_final BEFORE UPDATE ON warm_reviewers
  WHEN OLD.released_at IS NOT NULL
  BEGIN SELECT RAISE(ABORT, 'a released warm reviewer is final'); END;
CREATE TRIGGER immutable_warm_reviewers_identity BEFORE UPDATE OF project_id, plan_id, reviewer_role, agent_id, created_at ON warm_reviewers
  BEGIN SELECT RAISE(ABORT, 'warm reviewer identity is immutable'); END;
CREATE TRIGGER immutable_warm_reviewers_delete BEFORE DELETE ON warm_reviewers BEGIN SELECT RAISE(ABORT, 'warm reviewers are immutable'); END;
