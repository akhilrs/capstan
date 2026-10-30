-- Integration details: what a merged integration brought (its plans and reports) and its impact (changed files, areas,
-- migrations, next steps), recorded once in the mutation that finishes the merge. One row per integration; a row is
-- never rewritten (the trigger refuses an update and a delete) and retention does not prune it. integrations rows are
-- never deleted, so the foreign key cannot break a prune. A future rebuild of the integrations table must keep this
-- table: it must copy or re-point these rows, as the earlier rebuilds did for the tables that reference integrations.
CREATE TABLE IF NOT EXISTS integration_details (
  project_id TEXT NOT NULL,
  integration_id TEXT NOT NULL,
  details_json TEXT NOT NULL CHECK (json_valid(details_json) AND length(CAST(details_json AS BLOB)) <= 16384),
  computed_at TEXT NOT NULL,
  PRIMARY KEY (project_id, integration_id),
  FOREIGN KEY (project_id, integration_id) REFERENCES integrations(project_id, integration_id)
) STRICT, WITHOUT ROWID;

CREATE TRIGGER IF NOT EXISTS immutable_integration_details_update BEFORE UPDATE ON integration_details
  BEGIN SELECT RAISE(ABORT, 'integration details are immutable'); END;
CREATE TRIGGER IF NOT EXISTS immutable_integration_details_delete BEFORE DELETE ON integration_details
  BEGIN SELECT RAISE(ABORT, 'integration details are immutable'); END;

-- The lookups that read the plans and runs of one integration.
CREATE INDEX IF NOT EXISTS plan_signoffs_by_integration ON plan_signoffs(project_id, integration_id, created_at);
CREATE INDEX IF NOT EXISTS verify_runs_by_integration ON verify_runs(project_id, subject_integration_id, sequence);
