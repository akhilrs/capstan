ALTER TABLE candidate_evidence
  ADD COLUMN observation TEXT;
ALTER TABLE candidate_evidence
  ADD COLUMN exit_status INTEGER CHECK (exit_status IS NULL OR exit_status BETWEEN 0 AND 255);

ALTER TABLE final_verification_evidence
  ADD COLUMN observation TEXT;
ALTER TABLE final_verification_evidence
  ADD COLUMN exit_status INTEGER CHECK (exit_status IS NULL OR exit_status BETWEEN 0 AND 255);
