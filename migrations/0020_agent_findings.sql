CREATE TABLE agent_findings (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  finding_id TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  target_agent_id TEXT NOT NULL,
  raised_by_agent_id TEXT NOT NULL,
  raised_by_actor_id TEXT NOT NULL,
  severity TEXT NOT NULL CHECK (severity IN ('info', 'low', 'medium', 'high', 'critical')),
  evidence_text TEXT NOT NULL CHECK (trim(evidence_text) <> ''),
  requested_correction TEXT NOT NULL CHECK (trim(requested_correction) <> ''),
  resolution_condition TEXT NOT NULL CHECK (trim(resolution_condition) <> ''),
  state TEXT NOT NULL CHECK (state IN ('open', 'resolved', 'escalated', 'cancelled')),
  interventions INTEGER NOT NULL CHECK (interventions IN (1, 2)),
  state_reason TEXT CHECK (state_reason IN ('second_unresolved', 'timed_out', 'target_ended', 'raiser_ended')),
  created_at TEXT NOT NULL,
  closed_at TEXT,
  PRIMARY KEY (project_id, finding_id),
  UNIQUE (project_id, sequence),
  FOREIGN KEY (project_id, target_agent_id) REFERENCES agents(project_id, agent_id),
  FOREIGN KEY (project_id, raised_by_agent_id) REFERENCES agents(project_id, agent_id),
  FOREIGN KEY (project_id, raised_by_actor_id) REFERENCES actors(project_id, actor_id),
  CHECK (raised_by_agent_id <> target_agent_id),
  CHECK ((state = 'open' AND closed_at IS NULL AND state_reason IS NULL)
      OR (state = 'resolved' AND closed_at IS NOT NULL AND state_reason IS NULL)
      OR (state = 'escalated' AND closed_at IS NOT NULL AND state_reason IN ('second_unresolved', 'timed_out'))
      OR (state = 'cancelled' AND closed_at IS NOT NULL AND state_reason IN ('target_ended', 'raiser_ended')))
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX one_open_finding_per_target ON agent_findings(project_id, target_agent_id) WHERE state = 'open';
CREATE INDEX agent_findings_by_raiser ON agent_findings(project_id, raised_by_agent_id, state);

CREATE TABLE agent_finding_deliveries (
  project_id TEXT NOT NULL,
  finding_id TEXT NOT NULL,
  attempt INTEGER NOT NULL CHECK (attempt IN (1, 2)),
  message_id TEXT NOT NULL,
  evidence_text TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, finding_id, attempt),
  UNIQUE (project_id, message_id),
  FOREIGN KEY (project_id, finding_id) REFERENCES agent_findings(project_id, finding_id),
  FOREIGN KEY (project_id, message_id) REFERENCES messages(project_id, message_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_finding_checks (
  project_id TEXT NOT NULL,
  finding_id TEXT NOT NULL,
  check_id TEXT NOT NULL,
  after_intervention INTEGER NOT NULL CHECK (after_intervention IN (1, 2)),
  result TEXT NOT NULL CHECK (result IN ('resolved', 'unresolved', 'timed_out')),
  evidence_text TEXT NOT NULL,
  checked_by_actor_id TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, check_id),
  UNIQUE (project_id, finding_id, after_intervention),
  FOREIGN KEY (project_id, finding_id) REFERENCES agent_findings(project_id, finding_id),
  FOREIGN KEY (project_id, checked_by_actor_id) REFERENCES actors(project_id, actor_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_finding_notices (
  project_id TEXT NOT NULL,
  notice_id TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  finding_id TEXT NOT NULL,
  event TEXT NOT NULL CHECK (event IN ('raised', 'resolved', 'escalated', 'cancelled')),
  message_id TEXT,
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, notice_id),
  UNIQUE (project_id, sequence),
  UNIQUE (project_id, finding_id, event),
  FOREIGN KEY (project_id, finding_id) REFERENCES agent_findings(project_id, finding_id),
  FOREIGN KEY (project_id, message_id) REFERENCES messages(project_id, message_id)
) STRICT, WITHOUT ROWID;

CREATE TRIGGER immutable_agent_findings_content BEFORE UPDATE OF project_id, finding_id, sequence, target_agent_id, raised_by_agent_id, raised_by_actor_id, severity, evidence_text, requested_correction, resolution_condition, created_at ON agent_findings BEGIN SELECT RAISE(ABORT, 'a finding is immutable'); END;
CREATE TRIGGER agent_findings_state_moves_forward BEFORE UPDATE OF state ON agent_findings
  WHEN NOT (OLD.state = 'open' AND NEW.state IN ('resolved', 'escalated', 'cancelled'))
  BEGIN SELECT RAISE(ABORT, 'a finding state only moves forward'); END;
CREATE TRIGGER agent_findings_final_is_final BEFORE UPDATE OF interventions, state_reason, closed_at ON agent_findings
  WHEN OLD.state <> 'open'
  BEGIN SELECT RAISE(ABORT, 'a closed finding is final'); END;
CREATE TRIGGER agent_findings_interventions_only_grow BEFORE UPDATE OF interventions ON agent_findings
  WHEN NOT (OLD.interventions = 1 AND NEW.interventions = 2)
  BEGIN SELECT RAISE(ABORT, 'interventions only grow from one to two'); END;
CREATE TRIGGER immutable_agent_findings_delete BEFORE DELETE ON agent_findings BEGIN SELECT RAISE(ABORT, 'findings are immutable'); END;
CREATE TRIGGER immutable_agent_finding_deliveries_update BEFORE UPDATE ON agent_finding_deliveries BEGIN SELECT RAISE(ABORT, 'finding deliveries are immutable'); END;
CREATE TRIGGER immutable_agent_finding_deliveries_delete BEFORE DELETE ON agent_finding_deliveries BEGIN SELECT RAISE(ABORT, 'finding deliveries are immutable'); END;
CREATE TRIGGER immutable_agent_finding_checks_update BEFORE UPDATE ON agent_finding_checks BEGIN SELECT RAISE(ABORT, 'finding checks are immutable'); END;
CREATE TRIGGER immutable_agent_finding_checks_delete BEFORE DELETE ON agent_finding_checks BEGIN SELECT RAISE(ABORT, 'finding checks are immutable'); END;
CREATE TRIGGER immutable_agent_finding_notices_content BEFORE UPDATE OF project_id, notice_id, sequence, finding_id, event, created_at ON agent_finding_notices BEGIN SELECT RAISE(ABORT, 'finding notices are immutable'); END;
CREATE TRIGGER agent_finding_notices_announced_once BEFORE UPDATE OF message_id ON agent_finding_notices WHEN OLD.message_id IS NOT NULL BEGIN SELECT RAISE(ABORT, 'a finding notice is announced once'); END;
CREATE TRIGGER immutable_agent_finding_notices_delete BEFORE DELETE ON agent_finding_notices BEGIN SELECT RAISE(ABORT, 'finding notices are immutable'); END;

INSERT INTO role_capabilities(role, capability) VALUES
  ('Supervisor', 'finding:raise'), ('Supervisor', 'finding:check'),
  ('Supervisor', 'agent:observe'), ('PM', 'agent:observe');

INSERT OR IGNORE INTO capability_grants(project_id, actor_id, capability, granted_by, granted_at)
SELECT a.project_id, a.actor_id, rc.capability, internal.actor_id, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM actors a
JOIN role_capabilities rc ON rc.role = a.role AND rc.capability IN ('finding:raise', 'finding:check', 'agent:observe')
JOIN actors internal ON internal.project_id = a.project_id AND internal.is_internal = 1
  AND internal.role = 'controller' AND internal.active = 1 AND internal.revoked_at IS NULL
WHERE a.active = 1 AND a.revoked_at IS NULL AND a.role IN ('Supervisor', 'PM');
