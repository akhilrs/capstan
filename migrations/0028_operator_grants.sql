CREATE TABLE operator_grants (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  grant_id TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  kind TEXT NOT NULL CHECK (kind IN ('exact', 'prefix')),
  text TEXT NOT NULL CHECK (trim(text) <> '' AND length(CAST(text AS BLOB)) <= 8192),
  command_sha TEXT NOT NULL CHECK (length(command_sha) = 64 AND command_sha NOT GLOB '*[^0-9a-f]*'),
  created_by TEXT NOT NULL,
  source_proposal_id TEXT NOT NULL,
  created_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  revoked_at TEXT,
  ended_reason TEXT CHECK (ended_reason IS NULL OR ended_reason IN ('released', 'restart', 'expired', 'revoked')),
  PRIMARY KEY (project_id, grant_id),
  UNIQUE (project_id, sequence),
  FOREIGN KEY (project_id, created_by) REFERENCES actors(project_id, actor_id),
  FOREIGN KEY (project_id, source_proposal_id) REFERENCES operator_proposals(project_id, proposal_id),
  CHECK (revoked_at IS NULL OR ended_reason = 'revoked')
) STRICT, WITHOUT ROWID;

CREATE INDEX operator_grants_open ON operator_grants(project_id, ended_reason);

CREATE TRIGGER operator_grants_identity_is_immutable BEFORE UPDATE OF project_id, grant_id, sequence, kind, text, command_sha, created_by, source_proposal_id, created_at, expires_at ON operator_grants
  BEGIN SELECT RAISE(ABORT, 'a grant text, kind, source and expiry are immutable'); END;
CREATE TRIGGER operator_grants_end_is_final BEFORE UPDATE OF revoked_at, ended_reason ON operator_grants
  WHEN OLD.ended_reason IS NOT NULL
  BEGIN SELECT RAISE(ABORT, 'a grant ends once'); END;
CREATE TRIGGER immutable_operator_grants_delete BEFORE DELETE ON operator_grants
  BEGIN SELECT RAISE(ABORT, 'operator grants are immutable'); END;

ALTER TABLE operator_runs ADD COLUMN full_auto INTEGER NOT NULL DEFAULT 0 CHECK (full_auto IN (0, 1));

CREATE TRIGGER operator_runs_full_auto_is_final BEFORE UPDATE OF full_auto ON operator_runs
  BEGIN SELECT RAISE(ABORT, 'the full auto mark of a run is final'); END;

DROP TRIGGER operator_proposals_start_undecided;
CREATE TRIGGER operator_proposals_start_undecided BEFORE INSERT ON operator_proposals
  WHEN NOT (NEW.state = 'proposed' AND NEW.auto_rule IS NULL AND NEW.decided_at IS NULL AND NEW.decided_by_actor_id IS NULL)
   AND NOT (NEW.state = 'approved' AND NEW.auto_rule IS NOT NULL AND NEW.decided_at IS NOT NULL AND NEW.decided_by_actor_id IS NULL
            AND (NEW.kind = 'command' OR NEW.auto_rule = 'full-auto'))
  BEGIN SELECT RAISE(ABORT, 'a proposal starts as proposed, or as approved by an auto-approve rule'); END;
