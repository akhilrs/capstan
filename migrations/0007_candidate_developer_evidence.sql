ALTER TABLE candidates
  ADD COLUMN evidence_json TEXT
  CHECK (evidence_json IS NULL OR (json_valid(evidence_json) AND json_type(evidence_json) = 'array'));
