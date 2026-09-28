ALTER TABLE candidate_evidence
  ADD COLUMN observation TEXT NOT NULL DEFAULT 'Legacy evidence: observation not recorded';
ALTER TABLE candidate_evidence
  ADD COLUMN exit_status INTEGER NOT NULL DEFAULT 0 CHECK (exit_status BETWEEN 0 AND 255);

ALTER TABLE final_verification_evidence
  ADD COLUMN observation TEXT NOT NULL DEFAULT 'Legacy evidence: observation not recorded';
ALTER TABLE final_verification_evidence
  ADD COLUMN exit_status INTEGER NOT NULL DEFAULT 0 CHECK (exit_status BETWEEN 0 AND 255);
