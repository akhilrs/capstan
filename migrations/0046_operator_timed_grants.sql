CREATE TABLE operator_grants_new (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  grant_id TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  kind TEXT NOT NULL CHECK (kind IN ('exact', 'prefix', 'timed')),
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

INSERT INTO operator_grants_new SELECT project_id, grant_id, sequence, kind, text, command_sha, created_by, source_proposal_id, created_at, expires_at, revoked_at, ended_reason FROM operator_grants;
DROP TABLE operator_grants;
ALTER TABLE operator_grants_new RENAME TO operator_grants;

CREATE INDEX operator_grants_open ON operator_grants(project_id, ended_reason);

CREATE TRIGGER operator_grants_identity_is_immutable BEFORE UPDATE OF project_id, grant_id, sequence, kind, text, command_sha, created_by, source_proposal_id, created_at, expires_at ON operator_grants
  BEGIN SELECT RAISE(ABORT, 'a grant text, kind, source and expiry are immutable'); END;
CREATE TRIGGER operator_grants_end_is_final BEFORE UPDATE OF revoked_at, ended_reason ON operator_grants
  WHEN OLD.ended_reason IS NOT NULL
  BEGIN SELECT RAISE(ABORT, 'a grant ends once'); END;
CREATE TRIGGER immutable_operator_grants_delete BEFORE DELETE ON operator_grants
  BEGIN SELECT RAISE(ABORT, 'operator grants are immutable'); END;
