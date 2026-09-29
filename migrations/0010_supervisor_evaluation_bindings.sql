CREATE TABLE supervisor_evaluations (
  project_id TEXT NOT NULL,
  target_epoch INTEGER NOT NULL CHECK (target_epoch >= 0),
  event_upper_sequence INTEGER NOT NULL CHECK (event_upper_sequence >= 0),
  assignment_id TEXT,
  generation INTEGER CHECK (generation IS NULL OR generation > 0),
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, target_epoch, event_upper_sequence),
  UNIQUE (project_id, assignment_id),
  FOREIGN KEY (project_id, assignment_id) REFERENCES assignments(project_id, assignment_id)
) STRICT, WITHOUT ROWID;
