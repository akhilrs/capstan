ALTER TABLE candidates
  ADD COLUMN evidence_json TEXT NOT NULL DEFAULT '[]'
  CHECK (json_valid(evidence_json) AND json_type(evidence_json) = 'array');
