CREATE TABLE pauses (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  pause_id TEXT NOT NULL,
  scope TEXT NOT NULL CHECK (scope IN ('run', 'agent')),
  agent_id TEXT CHECK (agent_id IS NULL OR trim(agent_id) <> ''),
  reason TEXT NOT NULL CHECK (length(reason) BETWEEN 1 AND 500),
  actor_id TEXT NOT NULL,
  paused_at TEXT NOT NULL,
  resumed_at TEXT,
  resume_reason TEXT CHECK (resume_reason IS NULL OR length(resume_reason) BETWEEN 1 AND 500),
  resumed_by TEXT,
  PRIMARY KEY (project_id, pause_id),
  FOREIGN KEY (project_id, agent_id) REFERENCES agents(project_id, agent_id),
  FOREIGN KEY (project_id, actor_id) REFERENCES actors(project_id, actor_id),
  FOREIGN KEY (project_id, resumed_by) REFERENCES actors(project_id, actor_id),
  CHECK ((scope = 'run') = (agent_id IS NULL)),
  CHECK ((resumed_at IS NULL) = (resumed_by IS NULL)),
  CHECK ((resumed_at IS NULL) = (resume_reason IS NULL))
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX pauses_one_open_per_scope ON pauses(project_id, scope, COALESCE(agent_id, '')) WHERE resumed_at IS NULL;
CREATE INDEX pauses_by_agent ON pauses(project_id, agent_id);

CREATE TRIGGER pauses_identity_is_immutable BEFORE UPDATE OF project_id, pause_id, scope, agent_id, reason, actor_id, paused_at ON pauses
  BEGIN SELECT RAISE(ABORT, 'a pause record keeps its scope, reason and actor'); END;
CREATE TRIGGER pauses_resume_is_final BEFORE UPDATE OF resumed_at, resume_reason, resumed_by ON pauses
  WHEN OLD.resumed_at IS NOT NULL
  BEGIN SELECT RAISE(ABORT, 'a resumed pause is final'); END;
CREATE TRIGGER pauses_no_delete BEFORE DELETE ON pauses
  BEGIN SELECT RAISE(ABORT, 'pauses are immutable'); END;

INSERT INTO role_capabilities(role, capability) VALUES ('PM', 'run:control');

INSERT INTO transition_rules(entity_type, from_state, to_state, role, capability) VALUES
  ('run_control', 'active', 'paused', 'PM', 'run:control'),
  ('run_control', 'paused', 'active', 'PM', 'run:control');

INSERT OR IGNORE INTO capability_grants(project_id, actor_id, capability, granted_by, granted_at)
SELECT a.project_id, a.actor_id, 'run:control', internal.actor_id, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM actors a
JOIN actors internal ON internal.project_id = a.project_id AND internal.is_internal = 1
  AND internal.role = 'controller' AND internal.active = 1 AND internal.revoked_at IS NULL
WHERE a.active = 1 AND a.revoked_at IS NULL AND a.role = 'PM';
