CREATE TABLE operator_proposals (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  proposal_id TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  kind TEXT NOT NULL CHECK (kind IN ('command', 'restart')),
  command TEXT NOT NULL CHECK (trim(command) <> '' AND length(CAST(command AS BLOB)) <= 8192),
  command_sha TEXT NOT NULL CHECK (length(command_sha) = 64 AND command_sha NOT GLOB '*[^0-9a-f]*'),
  reason TEXT NOT NULL CHECK (trim(reason) <> '' AND length(CAST(reason AS BLOB)) <= 1024),
  force_restart INTEGER NOT NULL CHECK (force_restart IN (0, 1)),
  proposer_agent_id TEXT NOT NULL,
  proposer_actor_id TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('proposed', 'approved', 'denied', 'cancelled', 'expired', 'running', 'finished', 'failed', 'timeout', 'abandoned')),
  auto_rule TEXT,
  decided_by_actor_id TEXT,
  decided_at TEXT,
  decision_note TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (project_id, proposal_id),
  UNIQUE (project_id, sequence),
  FOREIGN KEY (project_id, proposer_agent_id) REFERENCES agents(project_id, agent_id),
  FOREIGN KEY (project_id, proposer_actor_id) REFERENCES actors(project_id, actor_id),
  FOREIGN KEY (project_id, decided_by_actor_id) REFERENCES actors(project_id, actor_id),
  CHECK (kind = 'restart' OR force_restart = 0),
  CHECK (decision_note IS NULL OR length(CAST(decision_note AS BLOB)) <= 1024)
) STRICT, WITHOUT ROWID;

CREATE INDEX operator_proposals_by_state ON operator_proposals(project_id, state);

CREATE TABLE operator_runs (
  project_id TEXT NOT NULL,
  proposal_id TEXT NOT NULL,
  started_at TEXT NOT NULL,
  finished_at TEXT,
  status TEXT NOT NULL CHECK (status IN ('running', 'ok', 'failed', 'timeout', 'error', 'abandoned')),
  exit_code INTEGER,
  duration_ms INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
  output_tail TEXT NOT NULL DEFAULT '' CHECK (length(CAST(output_tail AS BLOB)) <= 12288),
  output_truncated INTEGER NOT NULL DEFAULT 0 CHECK (output_truncated IN (0, 1)),
  notified_message_id TEXT,
  pgid INTEGER CHECK (pgid IS NULL OR pgid > 1),
  leader_start TEXT,
  orphan_cleared_at TEXT,
  PRIMARY KEY (project_id, proposal_id),
  FOREIGN KEY (project_id, proposal_id) REFERENCES operator_proposals(project_id, proposal_id),
  CHECK ((status = 'running') = (finished_at IS NULL))
) STRICT, WITHOUT ROWID;

CREATE TRIGGER operator_proposals_start_undecided BEFORE INSERT ON operator_proposals
  WHEN NOT (NEW.state = 'proposed' AND NEW.auto_rule IS NULL AND NEW.decided_at IS NULL AND NEW.decided_by_actor_id IS NULL)
   AND NOT (NEW.state = 'approved' AND NEW.auto_rule IS NOT NULL AND NEW.decided_at IS NOT NULL AND NEW.decided_by_actor_id IS NULL AND NEW.kind = 'command')
  BEGIN SELECT RAISE(ABORT, 'a proposal starts as proposed, or as approved by an auto-approve rule'); END;
CREATE TRIGGER operator_proposals_identity_is_immutable BEFORE UPDATE OF project_id, proposal_id, sequence, kind, command, command_sha, reason, force_restart, proposer_agent_id, proposer_actor_id, auto_rule, created_at ON operator_proposals
  BEGIN SELECT RAISE(ABORT, 'a proposal command, hash, reason, force flag and proposer are immutable'); END;
CREATE TRIGGER operator_proposals_decision_is_final BEFORE UPDATE OF decided_by_actor_id, decided_at, decision_note ON operator_proposals
  WHEN OLD.decided_at IS NOT NULL
  BEGIN SELECT RAISE(ABORT, 'a recorded decision is final'); END;
CREATE TRIGGER operator_proposals_move_forward BEFORE UPDATE OF state ON operator_proposals
  WHEN NOT ((OLD.state = 'proposed' AND NEW.state IN ('approved', 'denied', 'cancelled', 'expired'))
         OR (OLD.state = 'approved' AND NEW.state IN ('running', 'cancelled', 'expired'))
         OR (OLD.state = 'running' AND NEW.state IN ('finished', 'failed', 'timeout', 'abandoned')))
  BEGIN SELECT RAISE(ABORT, 'a proposal state only moves forward'); END;
CREATE TRIGGER operator_proposals_approval_needs_a_pm BEFORE UPDATE OF state ON operator_proposals
  WHEN NEW.state = 'approved'
   AND NOT (NEW.decided_at IS NOT NULL
        AND (NEW.auto_rule IS NOT NULL
          OR (NEW.decided_by_actor_id IS NOT NULL
              AND NEW.decided_by_actor_id <> NEW.proposer_actor_id
              AND EXISTS (SELECT 1 FROM agents a WHERE a.project_id = NEW.project_id AND a.actor_id = NEW.decided_by_actor_id AND a.kind = 'PM'))))
  BEGIN SELECT RAISE(ABORT, 'a proposal is approved by a PM agent other than its proposer, or by an auto-approve rule'); END;
CREATE TRIGGER operator_proposals_denial_is_decided BEFORE UPDATE OF state ON operator_proposals
  WHEN NEW.state = 'denied' AND (NEW.decided_at IS NULL OR NEW.decided_by_actor_id IS NULL)
  BEGIN SELECT RAISE(ABORT, 'a denial records who decided and when'); END;
CREATE TRIGGER immutable_operator_proposals_delete BEFORE DELETE ON operator_proposals
  BEGIN SELECT RAISE(ABORT, 'operator proposals are immutable'); END;

CREATE TRIGGER operator_runs_only_for_running_proposals BEFORE INSERT ON operator_runs
  WHEN NEW.status <> 'running'
    OR (SELECT state FROM operator_proposals WHERE project_id = NEW.project_id AND proposal_id = NEW.proposal_id) IS NOT 'running'
  BEGIN SELECT RAISE(ABORT, 'a run row exists only for a proposal that is running or later'); END;
CREATE TRIGGER operator_runs_identity_is_immutable BEFORE UPDATE OF project_id, proposal_id, started_at ON operator_runs
  BEGIN SELECT RAISE(ABORT, 'run identity is immutable'); END;
CREATE TRIGGER operator_runs_result_is_final BEFORE UPDATE OF status, finished_at, exit_code, duration_ms, output_tail, output_truncated ON operator_runs
  WHEN OLD.status <> 'running'
  BEGIN SELECT RAISE(ABORT, 'a finished run is final'); END;
CREATE TRIGGER operator_runs_notified_once BEFORE UPDATE OF notified_message_id ON operator_runs
  WHEN OLD.notified_message_id IS NOT NULL
  BEGIN SELECT RAISE(ABORT, 'a run is announced once'); END;
CREATE TRIGGER operator_runs_process_is_final BEFORE UPDATE OF pgid, leader_start ON operator_runs
  WHEN OLD.pgid IS NOT NULL
  BEGIN SELECT RAISE(ABORT, 'the recorded process of a run is final'); END;
CREATE TRIGGER operator_runs_orphan_cleared_once BEFORE UPDATE OF orphan_cleared_at ON operator_runs
  WHEN OLD.orphan_cleared_at IS NOT NULL
  BEGIN SELECT RAISE(ABORT, 'an orphan is cleared once'); END;
CREATE TRIGGER immutable_operator_runs_delete BEFORE DELETE ON operator_runs
  BEGIN SELECT RAISE(ABORT, 'operator runs are immutable'); END;

INSERT INTO role_capabilities(role, capability) VALUES
  ('Developer', 'operator:propose'),
  ('PM', 'operator:decide'), ('operator', 'operator:decide'),
  ('PM', 'operator:read'), ('operator', 'operator:read'), ('Developer', 'operator:read');

INSERT OR IGNORE INTO capability_grants(project_id, actor_id, capability, granted_by, granted_at)
SELECT a.project_id, a.actor_id, rc.capability, internal.actor_id, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM actors a
JOIN role_capabilities rc ON rc.role = a.role AND rc.capability IN ('operator:propose', 'operator:decide', 'operator:read')
JOIN actors internal ON internal.project_id = a.project_id AND internal.is_internal = 1
  AND internal.role = 'controller' AND internal.active = 1 AND internal.revoked_at IS NULL
WHERE a.active = 1 AND a.revoked_at IS NULL AND a.role IN ('operator', 'PM', 'Developer');
