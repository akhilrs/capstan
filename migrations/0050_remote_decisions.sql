-- Remote decisions: what a remote client asked for on a request id, one row per nonce. A row ties the decision to the
-- content hash that was current when it was made, so a repeat of the same nonce is answered from the row. Kept on purpose as
-- an audit record (retention and cleanups do not prune it). A forwarded row is replaced by a newer forward of the same
-- request (superseded_by) or closed by an applied decision.
CREATE TABLE IF NOT EXISTS remote_decisions (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  nonce TEXT NOT NULL,
  decision_id TEXT NOT NULL,
  request_id TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  verb TEXT NOT NULL,
  args_json TEXT NOT NULL,
  client TEXT NOT NULL,
  outcome TEXT NOT NULL CHECK (outcome IN ('applied', 'forwarded')),
  result_json TEXT NOT NULL,
  message_id TEXT,
  superseded_by TEXT,
  decided_at TEXT NOT NULL,
  PRIMARY KEY (project_id, nonce),
  UNIQUE (project_id, decision_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS remote_decisions_by_request ON remote_decisions(project_id, request_id, outcome);
