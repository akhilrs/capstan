-- Package landings: the operator's statement that the content of a package is on HEAD although no integration of the
-- ledger holds its report. The row belongs to the package, not to its assignee, so it keeps counting after a
-- reassignment. A row is never updated: DELETE is the clear operation.
CREATE TABLE IF NOT EXISTS package_landings (
  project_id TEXT NOT NULL,
  plan_id TEXT NOT NULL,
  package_id TEXT NOT NULL,
  commit_sha TEXT NOT NULL CHECK (length(commit_sha) = 40),
  recorded_by TEXT NOT NULL,
  recorded_at TEXT NOT NULL,
  note TEXT NOT NULL CHECK (length(note) BETWEEN 1 AND 500),
  PRIMARY KEY (project_id, plan_id, package_id),
  FOREIGN KEY (project_id, plan_id, package_id) REFERENCES plan_packages(project_id, plan_id, package_id)
) STRICT, WITHOUT ROWID;

CREATE TRIGGER IF NOT EXISTS package_landings_immutable BEFORE UPDATE ON package_landings
BEGIN
  SELECT RAISE(ABORT, 'a package landing is cleared and recorded again, never updated');
END;
