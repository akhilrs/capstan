-- The steps of a verify run that were not run because the diff touched none of the paths of their `[verify]` filter:
-- a JSON array of { step, command, reason }, written once when the run finishes. Runs made before this column, and
-- runs without a path filter, hold [].
ALTER TABLE verify_runs ADD COLUMN skipped_json TEXT NOT NULL DEFAULT '[]';
