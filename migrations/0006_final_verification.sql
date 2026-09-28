ALTER TABLE work_items ADD COLUMN final_verification INTEGER NOT NULL DEFAULT 0 CHECK (final_verification IN (0, 1));

CREATE TABLE final_verification_commits (
  project_id TEXT NOT NULL,
  work_item_id TEXT NOT NULL,
  assignment_id TEXT NOT NULL,
  input_revision INTEGER NOT NULL,
  commit_sha TEXT NOT NULL,
  created_by TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, work_item_id),
  UNIQUE (project_id, work_item_id, assignment_id, input_revision, commit_sha),
  FOREIGN KEY (project_id, work_item_id) REFERENCES work_items(project_id, work_item_id),
  FOREIGN KEY (project_id, assignment_id) REFERENCES assignments(project_id, assignment_id)
);

CREATE TABLE final_verification_evidence (
  project_id TEXT NOT NULL,
  work_item_id TEXT NOT NULL,
  assignment_id TEXT NOT NULL,
  evidence_id TEXT NOT NULL,
  input_revision INTEGER NOT NULL,
  commit_sha TEXT NOT NULL,
  criterion TEXT NOT NULL,
  passed INTEGER NOT NULL CHECK (passed = 1),
  artifact_ref TEXT NOT NULL,
  evidence_hash TEXT NOT NULL,
  created_by TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, work_item_id, criterion),
  UNIQUE (project_id, evidence_id),
  FOREIGN KEY (project_id, work_item_id) REFERENCES work_items(project_id, work_item_id),
  FOREIGN KEY (project_id, assignment_id) REFERENCES assignments(project_id, assignment_id),
  FOREIGN KEY (project_id, work_item_id, assignment_id, input_revision, commit_sha)
    REFERENCES final_verification_commits(project_id, work_item_id, assignment_id, input_revision, commit_sha)
);
