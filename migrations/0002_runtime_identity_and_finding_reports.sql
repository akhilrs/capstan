ALTER TABLE finding_responses RENAME TO finding_responses_v1;

CREATE TABLE finding_responses (
  project_id TEXT NOT NULL,
  response_id TEXT NOT NULL,
  finding_id TEXT NOT NULL,
  assignment_id TEXT NOT NULL,
  response_type TEXT NOT NULL CHECK (response_type IN ('report', 'acknowledged', 'correction', 'dispute')),
  content_json TEXT NOT NULL,
  created_by TEXT NOT NULL REFERENCES actors(actor_id),
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, response_id),
  FOREIGN KEY (project_id, finding_id) REFERENCES findings(project_id, finding_id),
  FOREIGN KEY (project_id, assignment_id) REFERENCES assignments(project_id, assignment_id)
) STRICT, WITHOUT ROWID;

INSERT INTO finding_responses(project_id, response_id, finding_id, assignment_id, response_type, content_json, created_by, created_at)
SELECT project_id, response_id, finding_id, assignment_id, response_type, content_json, created_by, created_at
FROM finding_responses_v1;
DROP TABLE finding_responses_v1;

ALTER TABLE runtime_identities RENAME TO runtime_identities_v1;

CREATE TABLE runtime_identities (
  project_id TEXT NOT NULL,
  observation_id TEXT NOT NULL,
  session_id TEXT NOT NULL,
  observed_pid INTEGER,
  process_start_id TEXT,
  container_id TEXT,
  cgroup_path TEXT,
  endpoint TEXT,
  observed_at TEXT NOT NULL,
  PRIMARY KEY (project_id, observation_id),
  FOREIGN KEY (project_id, session_id) REFERENCES runtime_sessions(project_id, session_id)
) STRICT, WITHOUT ROWID;

INSERT INTO runtime_identities(project_id, observation_id, session_id, observed_pid, process_start_id, container_id,
  cgroup_path, endpoint, observed_at)
SELECT project_id, lower(hex(randomblob(16))), session_id, observed_pid, process_start_id, container_id,
  cgroup_path, endpoint, observed_at
FROM runtime_identities_v1;
DROP TABLE runtime_identities_v1;

INSERT INTO transition_rules(entity_type, from_state, to_state, role, capability)
VALUES ('work_item', 'blocked', 'awaiting_verification', 'controller', 'controller:reconcile');