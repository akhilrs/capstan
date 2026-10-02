CREATE TABLE plans (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  plan_id TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  title TEXT NOT NULL CHECK (trim(title) <> ''),
  tier TEXT NOT NULL CHECK (tier IN ('normal', 'high_risk')),
  state TEXT NOT NULL CHECK (state IN ('draft', 'in_review', 'approved', 'superseded')),
  requested_by TEXT NOT NULL,
  architect_agent_id TEXT,
  current_revision INTEGER NOT NULL DEFAULT 0 CHECK (current_revision >= 0),
  approved_revision INTEGER,
  supersedes_plan_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (project_id, plan_id),
  UNIQUE (project_id, sequence),
  FOREIGN KEY (project_id, architect_agent_id) REFERENCES agents(project_id, agent_id),
  FOREIGN KEY (project_id, supersedes_plan_id) REFERENCES plans(project_id, plan_id),
  CHECK (plan_id <> supersedes_plan_id),
  CHECK ((state IN ('approved', 'superseded')) = (approved_revision IS NOT NULL)),
  CHECK (approved_revision IS NULL OR (approved_revision >= 1 AND approved_revision <= current_revision)),
  CHECK (state = 'draft' OR current_revision >= 1)
) STRICT, WITHOUT ROWID;

CREATE INDEX plans_by_state ON plans(project_id, state);

CREATE TABLE plan_revisions (
  project_id TEXT NOT NULL,
  plan_id TEXT NOT NULL,
  revision INTEGER NOT NULL CHECK (revision >= 1),
  base_sha TEXT NOT NULL CHECK (length(base_sha) = 40 AND base_sha NOT GLOB '*[^0-9a-f]*'),
  body_json TEXT NOT NULL CHECK (json_valid(body_json) AND json_type(body_json) = 'object' AND length(CAST(body_json AS BLOB)) <= 32768),
  body_sha TEXT NOT NULL CHECK (length(body_sha) = 64),
  author_agent_id TEXT NOT NULL,
  author_actor_id TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, plan_id, revision),
  FOREIGN KEY (project_id, plan_id) REFERENCES plans(project_id, plan_id),
  FOREIGN KEY (project_id, author_agent_id) REFERENCES agents(project_id, agent_id),
  FOREIGN KEY (project_id, author_actor_id) REFERENCES actors(project_id, actor_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE plan_packages (
  project_id TEXT NOT NULL,
  plan_id TEXT NOT NULL,
  package_id TEXT NOT NULL CHECK (package_id GLOB '[a-z]*' AND length(package_id) <= 32 AND package_id NOT GLOB '*[^a-z0-9-]*'),
  assignee_agent_id TEXT,
  assigned_at TEXT,
  assignment_message_id TEXT,
  PRIMARY KEY (project_id, plan_id, package_id),
  FOREIGN KEY (project_id, plan_id) REFERENCES plans(project_id, plan_id),
  FOREIGN KEY (project_id, assignee_agent_id) REFERENCES agents(project_id, agent_id),
  FOREIGN KEY (project_id, assignment_message_id) REFERENCES messages(project_id, message_id),
  CHECK ((assignee_agent_id IS NULL) = (assigned_at IS NULL))
) STRICT, WITHOUT ROWID;

CREATE INDEX plan_packages_by_assignee ON plan_packages(project_id, assignee_agent_id);

CREATE TABLE plan_signoffs (
  project_id TEXT NOT NULL,
  plan_id TEXT NOT NULL,
  integration_id TEXT NOT NULL,
  architect_agent_id TEXT NOT NULL,
  summary TEXT NOT NULL CHECK (trim(summary) <> ''),
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, plan_id, integration_id),
  FOREIGN KEY (project_id, plan_id) REFERENCES plans(project_id, plan_id),
  FOREIGN KEY (project_id, integration_id) REFERENCES integrations(project_id, integration_id),
  FOREIGN KEY (project_id, architect_agent_id) REFERENCES agents(project_id, agent_id)
) STRICT, WITHOUT ROWID;

CREATE TRIGGER plans_identity_is_immutable BEFORE UPDATE OF project_id, plan_id, sequence, title, tier, requested_by, supersedes_plan_id, created_at ON plans
  BEGIN SELECT RAISE(ABORT, 'plan identity is immutable'); END;
CREATE TRIGGER plans_move_forward BEFORE UPDATE OF state ON plans
  WHEN NOT ((OLD.state = 'draft' AND NEW.state IN ('in_review', 'approved'))
         OR (OLD.state = 'in_review' AND NEW.state IN ('approved', 'draft'))
         OR (OLD.state = 'approved' AND NEW.state = 'superseded'))
  BEGIN SELECT RAISE(ABORT, 'a plan state only moves forward'); END;
CREATE TRIGGER plans_revision_counts_up BEFORE UPDATE OF current_revision ON plans
  WHEN NEW.current_revision <> OLD.current_revision + 1
  BEGIN SELECT RAISE(ABORT, 'a plan revision only counts up by one'); END;
CREATE TRIGGER plans_approved_revision_is_final BEFORE UPDATE OF approved_revision ON plans
  WHEN OLD.approved_revision IS NOT NULL AND NEW.approved_revision IS NOT OLD.approved_revision
  BEGIN SELECT RAISE(ABORT, 'the approved revision is final'); END;
CREATE TRIGGER plans_architect_is_final BEFORE UPDATE OF architect_agent_id ON plans
  WHEN OLD.architect_agent_id IS NOT NULL AND NEW.architect_agent_id IS NOT OLD.architect_agent_id
  BEGIN SELECT RAISE(ABORT, 'the plan architect is final'); END;
CREATE TRIGGER plans_supersede_only_approved BEFORE INSERT ON plans
  WHEN NEW.supersedes_plan_id IS NOT NULL AND (SELECT state FROM plans WHERE project_id = NEW.project_id AND plan_id = NEW.supersedes_plan_id) IS NOT 'approved'
  BEGIN SELECT RAISE(ABORT, 'only an approved plan can be superseded'); END;
CREATE TRIGGER immutable_plans_delete BEFORE DELETE ON plans
  BEGIN SELECT RAISE(ABORT, 'plans are immutable'); END;

CREATE TRIGGER plan_revisions_follow_the_plan BEFORE INSERT ON plan_revisions
  WHEN NEW.revision IS NOT (SELECT current_revision + 1 FROM plans WHERE project_id = NEW.project_id AND plan_id = NEW.plan_id)
    OR (SELECT state FROM plans WHERE project_id = NEW.project_id AND plan_id = NEW.plan_id) IS NOT 'draft'
  BEGIN SELECT RAISE(ABORT, 'a revision can only be added to a draft plan as the next revision'); END;
CREATE TRIGGER immutable_plan_revisions_update BEFORE UPDATE ON plan_revisions
  BEGIN SELECT RAISE(ABORT, 'plan revisions are immutable'); END;
CREATE TRIGGER immutable_plan_revisions_delete BEFORE DELETE ON plan_revisions
  BEGIN SELECT RAISE(ABORT, 'plan revisions are immutable'); END;

CREATE TRIGGER plan_packages_only_for_the_approved_revision BEFORE INSERT ON plan_packages
  WHEN NOT EXISTS (
    SELECT 1 FROM plans p
    JOIN plan_revisions r ON r.project_id = p.project_id AND r.plan_id = p.plan_id AND r.revision = p.approved_revision
    JOIN json_each(r.body_json, '$.packages') j
    WHERE p.project_id = NEW.project_id AND p.plan_id = NEW.plan_id AND p.state = 'approved'
      AND json_extract(j.value, '$.id') = NEW.package_id)
  BEGIN SELECT RAISE(ABORT, 'a package row belongs to a package of the approved revision'); END;
CREATE TRIGGER plan_packages_identity_is_immutable BEFORE UPDATE OF project_id, plan_id, package_id ON plan_packages
  BEGIN SELECT RAISE(ABORT, 'package identity is immutable'); END;
CREATE TRIGGER immutable_plan_packages_delete BEFORE DELETE ON plan_packages
  BEGIN SELECT RAISE(ABORT, 'plan packages are immutable'); END;

CREATE TRIGGER immutable_plan_signoffs_update BEFORE UPDATE ON plan_signoffs
  BEGIN SELECT RAISE(ABORT, 'plan signoffs are immutable'); END;
CREATE TRIGGER immutable_plan_signoffs_delete BEFORE DELETE ON plan_signoffs
  BEGIN SELECT RAISE(ABORT, 'plan signoffs are immutable'); END;

INSERT INTO role_capabilities(role, capability) VALUES
  ('operator', 'plan:write'), ('operator', 'plan:read'),
  ('PM', 'plan:write'), ('PM', 'plan:read'),
  ('Developer', 'plan:write'), ('Developer', 'plan:read'),
  ('Verifier', 'plan:read');

INSERT OR IGNORE INTO capability_grants(project_id, actor_id, capability, granted_by, granted_at)
SELECT a.project_id, a.actor_id, rc.capability, internal.actor_id, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM actors a
JOIN role_capabilities rc ON rc.role = a.role AND rc.capability IN ('plan:write', 'plan:read')
JOIN actors internal ON internal.project_id = a.project_id AND internal.is_internal = 1
  AND internal.role = 'controller' AND internal.active = 1 AND internal.revoked_at IS NULL
WHERE a.active = 1 AND a.revoked_at IS NULL AND a.role IN ('operator', 'PM', 'Developer', 'Verifier');
