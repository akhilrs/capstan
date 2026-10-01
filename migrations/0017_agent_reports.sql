CREATE TABLE agent_reports (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  report_id TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  agent_id TEXT NOT NULL,
  generation INTEGER NOT NULL CHECK (generation >= 1),
  actor_id TEXT NOT NULL,
  commit_sha TEXT NOT NULL CHECK (length(commit_sha) = 40),
  branch TEXT,
  summary TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('accepted', 'rejected')),
  reason TEXT,
  evidence_json TEXT NOT NULL,
  notified_message_id TEXT,
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, report_id),
  UNIQUE (project_id, sequence),
  FOREIGN KEY (project_id, agent_id) REFERENCES agents(project_id, agent_id),
  FOREIGN KEY (project_id, actor_id) REFERENCES actors(project_id, actor_id),
  CHECK ((state = 'accepted' AND reason IS NULL) OR (state = 'rejected' AND reason IS NOT NULL))
) STRICT, WITHOUT ROWID;

CREATE INDEX agent_reports_by_agent ON agent_reports(project_id, agent_id, generation, state);

CREATE TRIGGER immutable_agent_reports_content BEFORE UPDATE OF report_id, sequence, agent_id, generation, actor_id, commit_sha, branch, summary, state, reason, evidence_json, created_at ON agent_reports BEGIN SELECT RAISE(ABORT, 'agent reports are immutable'); END;
CREATE TRIGGER agent_reports_notified_once BEFORE UPDATE OF notified_message_id ON agent_reports WHEN OLD.notified_message_id IS NOT NULL BEGIN SELECT RAISE(ABORT, 'a report is announced once'); END;
CREATE TRIGGER immutable_agent_reports_delete BEFORE DELETE ON agent_reports BEGIN SELECT RAISE(ABORT, 'agent reports are immutable'); END;

INSERT INTO role_capabilities(role, capability) VALUES
  ('Developer', 'report:submit'), ('Verifier', 'report:submit');

INSERT OR IGNORE INTO capability_grants(project_id, actor_id, capability, granted_by, granted_at)
SELECT a.project_id, a.actor_id, rc.capability, internal.actor_id, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM actors a
JOIN role_capabilities rc ON rc.role = a.role AND rc.capability = 'report:submit'
JOIN actors internal ON internal.project_id = a.project_id AND internal.is_internal = 1
  AND internal.role = 'controller' AND internal.active = 1 AND internal.revoked_at IS NULL
WHERE a.active = 1 AND a.revoked_at IS NULL AND a.role IN ('Developer', 'Verifier');
