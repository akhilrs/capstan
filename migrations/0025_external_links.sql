-- The PM mirrors work into Nexora and records here what it wrote. Nothing in the controller reads Nexora or decides a
-- transition from this table; the wanted state is derived from ledger facts and compared with synced_state to show drift.
CREATE TABLE external_links (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  ref_kind TEXT NOT NULL CHECK (ref_kind IN ('requirement', 'plan', 'package')),
  ref_id TEXT NOT NULL CHECK (trim(ref_id) <> ''),
  system TEXT NOT NULL CHECK (system IN ('nexora')),
  external_id TEXT NOT NULL CHECK (trim(external_id) <> ''),
  synced_state TEXT NOT NULL CHECK (synced_state IN ('backlog', 'todo', 'in_progress', 'in_review', 'completed', 'wont_do')),
  bound_agent_id TEXT,
  bound_at TEXT,
  linked_by TEXT NOT NULL,
  linked_at TEXT NOT NULL,
  synced_at TEXT NOT NULL,
  PRIMARY KEY (project_id, ref_kind, ref_id, system),
  FOREIGN KEY (project_id, bound_agent_id) REFERENCES agents(project_id, agent_id),
  FOREIGN KEY (project_id, linked_by) REFERENCES actors(project_id, actor_id),
  CHECK (bound_agent_id IS NULL OR ref_kind = 'requirement'),
  CHECK ((bound_agent_id IS NULL) = (bound_at IS NULL))
) STRICT, WITHOUT ROWID;

CREATE TRIGGER external_links_identity BEFORE UPDATE OF project_id, ref_kind, ref_id, system, external_id, linked_by, linked_at ON external_links
  BEGIN SELECT RAISE(ABORT, 'external link identity is immutable'); END;
CREATE TRIGGER external_links_no_delete BEFORE DELETE ON external_links
  BEGIN SELECT RAISE(ABORT, 'external links are not deleted'); END;

ALTER TABLE plans ADD COLUMN cancelled_at TEXT;
ALTER TABLE plan_packages ADD COLUMN cancelled_at TEXT;

CREATE TRIGGER plans_cancel_once BEFORE UPDATE OF cancelled_at ON plans
  WHEN OLD.cancelled_at IS NOT NULL
  BEGIN SELECT RAISE(ABORT, 'a cancelled plan stays cancelled'); END;
CREATE TRIGGER plans_frozen_after_cancel BEFORE UPDATE OF state, current_revision, approved_revision ON plans
  WHEN OLD.cancelled_at IS NOT NULL
  BEGIN SELECT RAISE(ABORT, 'a cancelled plan cannot change'); END;
CREATE TRIGGER plan_packages_cancel_once BEFORE UPDATE OF cancelled_at ON plan_packages
  WHEN OLD.cancelled_at IS NOT NULL
  BEGIN SELECT RAISE(ABORT, 'a cancelled package stays cancelled'); END;
CREATE TRIGGER plan_packages_frozen_after_cancel BEFORE UPDATE OF assignee_agent_id, assigned_at, assignment_message_id ON plan_packages
  WHEN OLD.cancelled_at IS NOT NULL
  BEGIN SELECT RAISE(ABORT, 'a cancelled package cannot be reassigned'); END;
