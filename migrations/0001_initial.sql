CREATE TABLE schema_migrations (
  version INTEGER PRIMARY KEY CHECK (version > 0),
  name TEXT NOT NULL UNIQUE,
  checksum TEXT NOT NULL CHECK (length(checksum) = 64),
  applied_at TEXT NOT NULL
) STRICT;

CREATE TABLE projects (
  project_id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  current_input_revision INTEGER NOT NULL CHECK (current_input_revision > 0),
  state_version INTEGER NOT NULL CHECK (state_version > 0),
  created_at TEXT NOT NULL
) STRICT;

CREATE TABLE actors (
  actor_id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  display_name TEXT NOT NULL,
  role TEXT NOT NULL CHECK (role IN ('operator', 'controller', 'PM', 'Developer', 'Verifier', 'Supervisor')),
  seat_id TEXT,
  credential_hash TEXT NOT NULL UNIQUE CHECK (length(credential_hash) = 64),
  active INTEGER NOT NULL CHECK (active IN (0, 1)),
  is_internal INTEGER NOT NULL DEFAULT 0 CHECK (is_internal IN (0, 1)),
  created_at TEXT NOT NULL,
  revoked_at TEXT,
  UNIQUE (project_id, actor_id),
  FOREIGN KEY (project_id, seat_id) REFERENCES seats(project_id, seat_id)
) STRICT;

CREATE TABLE role_capabilities (
  role TEXT NOT NULL CHECK (role IN ('operator', 'controller', 'PM', 'Developer', 'Verifier', 'Supervisor')),
  capability TEXT NOT NULL,
  PRIMARY KEY (role, capability)
) STRICT, WITHOUT ROWID;

CREATE TABLE capability_grants (
  project_id TEXT NOT NULL,
  actor_id TEXT NOT NULL,
  capability TEXT NOT NULL,
  granted_by TEXT NOT NULL REFERENCES actors(actor_id),
  granted_at TEXT NOT NULL,
  revoked_at TEXT,
  PRIMARY KEY (project_id, actor_id, capability),
  FOREIGN KEY (project_id, actor_id) REFERENCES actors(project_id, actor_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE project_revisions (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  revision INTEGER NOT NULL CHECK (revision > 0),
  kind TEXT NOT NULL CHECK (kind IN ('project_config', 'task_brief', 'acceptance_criteria', 'policy', 'plan')),
  content_json TEXT NOT NULL,
  content_hash TEXT NOT NULL CHECK (length(content_hash) = 64),
  created_by TEXT NOT NULL REFERENCES actors(actor_id),
  request_id TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, revision, kind)
) STRICT, WITHOUT ROWID;

CREATE TABLE seats (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  seat_id TEXT NOT NULL,
  name TEXT NOT NULL,
  role TEXT NOT NULL CHECK (role IN ('PM', 'Developer', 'Verifier', 'Supervisor')),
  state TEXT NOT NULL CHECK (state IN ('active', 'disabled')),
  state_version INTEGER NOT NULL CHECK (state_version >= 0),
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, seat_id),
  UNIQUE (project_id, role, name)
) STRICT, WITHOUT ROWID;

CREATE TABLE work_items (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  work_item_id TEXT NOT NULL,
  parent_work_item_id TEXT,
  title TEXT NOT NULL,
  description TEXT NOT NULL,
  required_role TEXT NOT NULL CHECK (required_role IN ('PM', 'Developer', 'Verifier', 'Supervisor')),
  state TEXT NOT NULL CHECK (state IN ('pending', 'ready', 'running', 'awaiting_verification', 'accepted', 'blocked', 'canceled', 'failed')),
  state_version INTEGER NOT NULL CHECK (state_version >= 0),
  input_revision INTEGER NOT NULL CHECK (input_revision > 0),
  accepted_candidate_id TEXT,
  created_by TEXT NOT NULL REFERENCES actors(actor_id),
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, work_item_id),
  FOREIGN KEY (project_id, parent_work_item_id) REFERENCES work_items(project_id, work_item_id),
  FOREIGN KEY (project_id, accepted_candidate_id) REFERENCES candidates(project_id, candidate_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE dependency_edges (
  project_id TEXT NOT NULL,
  work_item_id TEXT NOT NULL,
  depends_on_work_item_id TEXT NOT NULL,
  required_candidate_id TEXT,
  created_by TEXT NOT NULL REFERENCES actors(actor_id),
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, work_item_id, depends_on_work_item_id),
  CHECK (work_item_id <> depends_on_work_item_id),
  FOREIGN KEY (project_id, work_item_id) REFERENCES work_items(project_id, work_item_id),
  FOREIGN KEY (project_id, depends_on_work_item_id) REFERENCES work_items(project_id, work_item_id),
  FOREIGN KEY (project_id, required_candidate_id) REFERENCES candidates(project_id, candidate_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE runtime_sessions (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  session_id TEXT NOT NULL,
  seat_id TEXT NOT NULL,
  assignment_id TEXT,
  state TEXT NOT NULL CHECK (state IN ('starting', 'ready', 'working', 'stopping', 'exited', 'unknown')),
  state_version INTEGER NOT NULL CHECK (state_version >= 0),
  provider TEXT NOT NULL,
  profile TEXT NOT NULL,
  workspace TEXT NOT NULL,
  started_at TEXT NOT NULL,
  ended_at TEXT,
  PRIMARY KEY (project_id, session_id),
  FOREIGN KEY (project_id, seat_id) REFERENCES seats(project_id, seat_id),
  FOREIGN KEY (project_id, assignment_id) REFERENCES assignments(project_id, assignment_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE runtime_identities (
  project_id TEXT NOT NULL,
  session_id TEXT NOT NULL,
  observed_pid INTEGER,
  process_start_id TEXT,
  container_id TEXT,
  cgroup_path TEXT,
  endpoint TEXT,
  observed_at TEXT NOT NULL,
  PRIMARY KEY (project_id, session_id, observed_at),
  FOREIGN KEY (project_id, session_id) REFERENCES runtime_sessions(project_id, session_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE assignments (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  assignment_id TEXT NOT NULL,
  work_item_id TEXT NOT NULL,
  seat_id TEXT NOT NULL,
  containment_proof_ref TEXT,
  state TEXT NOT NULL CHECK (state IN ('created', 'dispatched', 'acknowledged', 'running', 'reported', 'completed', 'revoked', 'failed')),
  state_version INTEGER NOT NULL CHECK (state_version >= 0),
  input_revision INTEGER NOT NULL CHECK (input_revision > 0),
  active_generation INTEGER NOT NULL CHECK (active_generation > 0),
  authority_state TEXT NOT NULL CHECK (authority_state IN ('active', 'contained', 'unknown', 'revoked')),
  created_by TEXT NOT NULL REFERENCES actors(actor_id),
  created_at TEXT NOT NULL,
  ended_at TEXT,
  PRIMARY KEY (project_id, assignment_id),
  FOREIGN KEY (project_id, work_item_id) REFERENCES work_items(project_id, work_item_id),
  FOREIGN KEY (project_id, seat_id) REFERENCES seats(project_id, seat_id)
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX one_active_writer_per_work_item
  ON assignments(project_id, work_item_id)
  WHERE authority_state IN ('active', 'unknown');

CREATE TABLE assignment_attempts (
  project_id TEXT NOT NULL,
  assignment_id TEXT NOT NULL,
  attempt INTEGER NOT NULL CHECK (attempt > 0),
  generation INTEGER NOT NULL CHECK (generation > 0),
  state TEXT NOT NULL CHECK (state IN ('created', 'dispatched', 'acknowledged', 'running', 'reported', 'completed', 'revoked', 'failed')),
  state_version INTEGER NOT NULL CHECK (state_version >= 0),
  authority_state TEXT NOT NULL CHECK (authority_state IN ('active', 'contained', 'unknown', 'revoked')),
  created_at TEXT NOT NULL,
  ended_at TEXT,
  PRIMARY KEY (project_id, assignment_id, attempt),
  UNIQUE (project_id, assignment_id, generation),
  FOREIGN KEY (project_id, assignment_id) REFERENCES assignments(project_id, assignment_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE assignment_input_bindings (
  project_id TEXT NOT NULL,
  assignment_id TEXT NOT NULL,
  input_revision INTEGER NOT NULL,
  input_kind TEXT NOT NULL CHECK (input_kind IN ('project_config', 'task_brief', 'acceptance_criteria', 'policy', 'plan', 'dependency_candidate', 'candidate')),
  source_revision INTEGER NOT NULL,
  source_id TEXT NOT NULL,
  content_hash TEXT NOT NULL CHECK (length(content_hash) = 64),
  bound_at TEXT NOT NULL,
  PRIMARY KEY (project_id, assignment_id, input_kind, source_id),
  FOREIGN KEY (project_id, assignment_id) REFERENCES assignments(project_id, assignment_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE commands (
  project_id TEXT NOT NULL,
  command_id TEXT NOT NULL,
  assignment_id TEXT NOT NULL,
  attempt INTEGER NOT NULL,
  generation INTEGER NOT NULL,
  command_type TEXT NOT NULL CHECK (command_type IN ('dispatch')),
  payload_json TEXT NOT NULL,
  payload_hash TEXT NOT NULL CHECK (length(payload_hash) = 64),
  state TEXT NOT NULL CHECK (state IN ('queued', 'attempting', 'acknowledged', 'started', 'completed', 'unknown', 'failed')),
  state_version INTEGER NOT NULL CHECK (state_version >= 0),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (project_id, command_id),
  UNIQUE (project_id, assignment_id, attempt, generation, command_type),
  FOREIGN KEY (project_id, assignment_id, attempt) REFERENCES assignment_attempts(project_id, assignment_id, attempt)
) STRICT, WITHOUT ROWID;

CREATE TABLE outbox_delivery_attempts (
  project_id TEXT NOT NULL,
  command_id TEXT NOT NULL,
  ordinal INTEGER NOT NULL CHECK (ordinal > 0),
  outcome TEXT NOT NULL CHECK (outcome IN ('attempting', 'acknowledged', 'unknown', 'completed', 'failed')),
  started_at TEXT NOT NULL,
  completed_at TEXT,
  response_json TEXT,
  PRIMARY KEY (project_id, command_id, ordinal),
  FOREIGN KEY (project_id, command_id) REFERENCES commands(project_id, command_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE command_receipts (
  project_id TEXT NOT NULL,
  role TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  command_id TEXT NOT NULL,
  assignment_id TEXT NOT NULL,
  attempt INTEGER NOT NULL,
  generation INTEGER NOT NULL,
  receipt_type TEXT NOT NULL CHECK (receipt_type IN ('accepted', 'submitted', 'working', 'tool_started', 'tool_completed', 'aborted', 'dispatch_error', 'completed', 'agent_end_without_reply')),
  receipt_json TEXT NOT NULL,
  receipt_hash TEXT NOT NULL CHECK (length(receipt_hash) = 64),
  received_at TEXT NOT NULL,
  PRIMARY KEY (project_id, role, sequence),
  UNIQUE (project_id, command_id, sequence),
  FOREIGN KEY (project_id, command_id) REFERENCES commands(project_id, command_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE candidates (
  project_id TEXT NOT NULL,
  candidate_id TEXT NOT NULL,
  assignment_id TEXT NOT NULL,
  attempt INTEGER NOT NULL,
  generation INTEGER NOT NULL,
  input_revision INTEGER NOT NULL,
  commit_sha TEXT NOT NULL,
  base_sha TEXT NOT NULL,
  changed_scope_json TEXT NOT NULL,
  limitations_json TEXT NOT NULL,
  report_hash TEXT NOT NULL CHECK (length(report_hash) = 64),
  created_by TEXT NOT NULL REFERENCES actors(actor_id),
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, candidate_id),
  UNIQUE (project_id, assignment_id, attempt, generation, commit_sha),
  FOREIGN KEY (project_id, assignment_id, attempt) REFERENCES assignment_attempts(project_id, assignment_id, attempt)
) STRICT, WITHOUT ROWID;

CREATE TABLE candidate_evidence (
  project_id TEXT NOT NULL,
  evidence_id TEXT NOT NULL,
  candidate_id TEXT NOT NULL,
  verifier_assignment_id TEXT NOT NULL,
  input_revision INTEGER NOT NULL,
  criterion TEXT NOT NULL,
  passed INTEGER NOT NULL CHECK (passed IN (0, 1)),
  artifact_ref TEXT NOT NULL,
  evidence_hash TEXT NOT NULL CHECK (length(evidence_hash) = 64),
  created_by TEXT NOT NULL REFERENCES actors(actor_id),
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, evidence_id),
  UNIQUE (project_id, candidate_id, verifier_assignment_id, criterion),
  FOREIGN KEY (project_id, candidate_id) REFERENCES candidates(project_id, candidate_id),
  FOREIGN KEY (project_id, verifier_assignment_id) REFERENCES assignments(project_id, assignment_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE findings (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  finding_id TEXT NOT NULL,
  work_item_id TEXT NOT NULL,
  assignment_id TEXT,
  generation INTEGER,
  fingerprint TEXT NOT NULL,
  severity TEXT NOT NULL CHECK (severity IN ('info', 'low', 'medium', 'high', 'critical')),
  evidence_json TEXT NOT NULL,
  requested_correction TEXT NOT NULL,
  resolution_condition TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('detected', 'reported', 'acknowledged', 'correcting', 'resolved', 'disputed', 'escalated')),
  state_version INTEGER NOT NULL CHECK (state_version >= 0),
  created_by TEXT NOT NULL REFERENCES actors(actor_id),
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, finding_id),
  FOREIGN KEY (project_id, work_item_id) REFERENCES work_items(project_id, work_item_id),
  FOREIGN KEY (project_id, assignment_id) REFERENCES assignments(project_id, assignment_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE finding_deliveries (
  project_id TEXT NOT NULL,
  finding_id TEXT NOT NULL,
  delivery_id TEXT NOT NULL,
  seat_id TEXT NOT NULL,
  command_id TEXT NOT NULL,
  delivered_at TEXT,
  acknowledged_at TEXT,
  PRIMARY KEY (project_id, delivery_id),
  UNIQUE (project_id, finding_id, seat_id),
  FOREIGN KEY (project_id, finding_id) REFERENCES findings(project_id, finding_id),
  FOREIGN KEY (project_id, seat_id) REFERENCES seats(project_id, seat_id),
  FOREIGN KEY (project_id, command_id) REFERENCES commands(project_id, command_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE finding_responses (
  project_id TEXT NOT NULL,
  response_id TEXT NOT NULL,
  finding_id TEXT NOT NULL,
  assignment_id TEXT NOT NULL,
  response_type TEXT NOT NULL CHECK (response_type IN ('acknowledged', 'correction', 'dispute')),
  content_json TEXT NOT NULL,
  created_by TEXT NOT NULL REFERENCES actors(actor_id),
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, response_id),
  FOREIGN KEY (project_id, finding_id) REFERENCES findings(project_id, finding_id),
  FOREIGN KEY (project_id, assignment_id) REFERENCES assignments(project_id, assignment_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE finding_dispositions (
  project_id TEXT NOT NULL,
  disposition_id TEXT NOT NULL,
  finding_id TEXT NOT NULL,
  disposition TEXT NOT NULL CHECK (disposition IN ('resolved', 'disputed', 'escalated')),
  content_json TEXT NOT NULL,
  created_by TEXT NOT NULL REFERENCES actors(actor_id),
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, disposition_id),
  FOREIGN KEY (project_id, finding_id) REFERENCES findings(project_id, finding_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE run_controls (
  project_id TEXT PRIMARY KEY REFERENCES projects(project_id),
  state TEXT NOT NULL CHECK (state IN ('active', 'paused', 'canceling', 'canceled', 'completed', 'failed')),
  state_version INTEGER NOT NULL CHECK (state_version >= 0),
  updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE transition_rules (
  entity_type TEXT NOT NULL CHECK (entity_type IN ('work_item', 'assignment_attempt', 'runtime_session', 'finding', 'run_control', 'command')),
  from_state TEXT NOT NULL,
  to_state TEXT NOT NULL,
  role TEXT NOT NULL CHECK (role IN ('operator', 'controller', 'PM', 'Developer', 'Verifier', 'Supervisor')),
  capability TEXT NOT NULL,
  PRIMARY KEY (entity_type, from_state, to_state, role, capability)
) STRICT, WITHOUT ROWID;

CREATE TABLE controller_events (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  event_id TEXT NOT NULL UNIQUE,
  entity_type TEXT NOT NULL,
  entity_id TEXT NOT NULL,
  from_state TEXT,
  to_state TEXT,
  state_version INTEGER NOT NULL CHECK (state_version >= 0),
  actor_id TEXT NOT NULL REFERENCES actors(actor_id),
  request_id TEXT NOT NULL,
  input_revision INTEGER NOT NULL,
  payload_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, sequence)
) STRICT, WITHOUT ROWID;

CREATE TABLE operator_actions (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  action_id TEXT NOT NULL,
  action_type TEXT NOT NULL,
  target_id TEXT NOT NULL,
  actor_id TEXT NOT NULL REFERENCES actors(actor_id),
  request_id TEXT NOT NULL,
  input_revision INTEGER NOT NULL,
  payload_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, action_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE recovery_attempts (
  project_id TEXT NOT NULL,
  recovery_id TEXT NOT NULL,
  work_item_id TEXT NOT NULL,
  assignment_id TEXT NOT NULL,
  recovery_type TEXT NOT NULL CHECK (recovery_type IN ('worker_replacement', 'finding_correction', 'implementation_remediation')),
  finding_id TEXT,
  generation INTEGER NOT NULL,
  reason TEXT NOT NULL,
  containment_state TEXT NOT NULL CHECK (containment_state IN ('unknown', 'contained')),
  containment_proof_ref TEXT,
  outcome TEXT NOT NULL CHECK (outcome IN ('pending', 'replacement_created', 'blocked', 'failed')),
  created_by TEXT NOT NULL REFERENCES actors(actor_id),
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, recovery_id),
  FOREIGN KEY (project_id, work_item_id) REFERENCES work_items(project_id, work_item_id),
  FOREIGN KEY (project_id, assignment_id) REFERENCES assignments(project_id, assignment_id),
  FOREIGN KEY (project_id, finding_id) REFERENCES findings(project_id, finding_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE usage_observations (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  observation_id TEXT NOT NULL,
  session_id TEXT,
  assignment_id TEXT,
  provider TEXT NOT NULL,
  metric TEXT NOT NULL,
  value REAL,
  availability TEXT NOT NULL CHECK (availability IN ('observed', 'unavailable', 'inferred')),
  detail_json TEXT NOT NULL,
  observed_at TEXT NOT NULL,
  created_by TEXT NOT NULL REFERENCES actors(actor_id),
  PRIMARY KEY (project_id, observation_id),
  FOREIGN KEY (project_id, session_id) REFERENCES runtime_sessions(project_id, session_id),
  FOREIGN KEY (project_id, assignment_id) REFERENCES assignments(project_id, assignment_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE mutation_requests (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  idempotency_key TEXT NOT NULL,
  request_id TEXT NOT NULL,
  actor_id TEXT NOT NULL REFERENCES actors(actor_id),
  request_hash TEXT NOT NULL CHECK (length(request_hash) = 64),
  result_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, idempotency_key),
  UNIQUE (project_id, request_id)
) STRICT, WITHOUT ROWID;

INSERT INTO role_capabilities(role, capability) VALUES
  ('operator', 'project:inputs:write'), ('operator', 'work:write'), ('operator', 'work:assign'),
  ('operator', 'work:report'), ('operator', 'candidate:verify'), ('operator', 'candidate:accept'),
  ('operator', 'finding:write'), ('operator', 'run:control'), ('operator', 'usage:write'),
  ('operator', 'actor:manage'), ('operator', 'recovery:write'), ('operator', 'controller:reconcile'),
  ('controller', 'work:assign'), ('controller', 'candidate:accept'), ('controller', 'run:control'),
  ('controller', 'finding:write'), ('controller', 'recovery:write'), ('controller', 'controller:reconcile'), ('controller', 'usage:write'),
  ('PM', 'project:inputs:write'), ('PM', 'work:write'), ('PM', 'finding:write'), ('PM', 'usage:write'),
  ('Developer', 'work:report'), ('Developer', 'finding:write'), ('Developer', 'usage:write'),
  ('Verifier', 'candidate:verify'), ('Verifier', 'work:report'), ('Verifier', 'finding:write'), ('Verifier', 'usage:write'),
  ('Supervisor', 'finding:write'), ('Supervisor', 'usage:write');

INSERT INTO transition_rules(entity_type, from_state, to_state, role, capability) VALUES
  ('work_item', 'pending', 'ready', 'controller', 'work:assign'),
  ('work_item', 'ready', 'running', 'controller', 'work:assign'),
  ('work_item', 'running', 'awaiting_verification', 'Developer', 'work:report'),
  ('work_item', 'running', 'awaiting_verification', 'Verifier', 'work:report'),
  ('work_item', 'awaiting_verification', 'accepted', 'controller', 'candidate:accept'),
  ('work_item', 'pending', 'blocked', 'controller', 'work:assign'),
  ('work_item', 'ready', 'blocked', 'controller', 'work:assign'),
  ('work_item', 'running', 'blocked', 'controller', 'work:assign'),
  ('work_item', 'awaiting_verification', 'blocked', 'controller', 'work:assign'),
  ('work_item', 'blocked', 'ready', 'controller', 'work:assign'),
  ('work_item', 'pending', 'canceled', 'operator', 'work:write'),
  ('work_item', 'ready', 'canceled', 'operator', 'work:write'),
  ('work_item', 'running', 'failed', 'controller', 'work:assign'),
  ('work_item', 'awaiting_verification', 'failed', 'controller', 'work:assign'),
  ('command', 'queued', 'attempting', 'controller', 'controller:reconcile'),
  ('command', 'attempting', 'acknowledged', 'controller', 'controller:reconcile'),
  ('command', 'attempting', 'attempting', 'controller', 'controller:reconcile'),
  ('command', 'attempting', 'unknown', 'controller', 'controller:reconcile'),
  ('command', 'attempting', 'failed', 'controller', 'controller:reconcile'),
  ('command', 'acknowledged', 'started', 'controller', 'controller:reconcile'),
  ('command', 'acknowledged', 'unknown', 'controller', 'controller:reconcile'),
  ('command', 'acknowledged', 'completed', 'controller', 'controller:reconcile'),
  ('command', 'started', 'completed', 'controller', 'controller:reconcile'),
  ('command', 'started', 'unknown', 'controller', 'controller:reconcile'),
  ('command', 'started', 'failed', 'controller', 'controller:reconcile'),
  ('assignment_attempt', 'dispatched', 'failed', 'controller', 'recovery:write'),
  ('assignment_attempt', 'created', 'dispatched', 'controller', 'controller:reconcile'),
  ('assignment_attempt', 'dispatched', 'acknowledged', 'controller', 'controller:reconcile'),
  ('assignment_attempt', 'acknowledged', 'running', 'controller', 'controller:reconcile'),
  ('assignment_attempt', 'running', 'reported', 'Developer', 'work:report'),
  ('assignment_attempt', 'running', 'reported', 'Verifier', 'work:report'),
  ('assignment_attempt', 'reported', 'completed', 'controller', 'candidate:accept'),
  ('assignment_attempt', 'created', 'revoked', 'controller', 'recovery:write'),
  ('assignment_attempt', 'dispatched', 'revoked', 'controller', 'recovery:write'),
  ('assignment_attempt', 'acknowledged', 'revoked', 'controller', 'recovery:write'),
  ('assignment_attempt', 'running', 'revoked', 'controller', 'recovery:write'),
  ('assignment_attempt', 'reported', 'revoked', 'controller', 'recovery:write'),
  ('assignment_attempt', 'running', 'failed', 'controller', 'recovery:write'),
  ('runtime_session', 'starting', 'ready', 'controller', 'controller:reconcile'),
  ('runtime_session', 'ready', 'working', 'controller', 'controller:reconcile'),
  ('runtime_session', 'working', 'stopping', 'controller', 'controller:reconcile'),
  ('runtime_session', 'stopping', 'exited', 'controller', 'controller:reconcile'),
  ('runtime_session', 'starting', 'unknown', 'controller', 'controller:reconcile'),
  ('runtime_session', 'ready', 'unknown', 'controller', 'controller:reconcile'),
  ('runtime_session', 'working', 'unknown', 'controller', 'controller:reconcile'),
  ('runtime_session', 'unknown', 'stopping', 'controller', 'recovery:write'),
  ('finding', 'detected', 'reported', 'Supervisor', 'finding:write'),
  ('finding', 'reported', 'acknowledged', 'PM', 'finding:write'),
  ('finding', 'reported', 'acknowledged', 'Developer', 'finding:write'),
  ('finding', 'reported', 'acknowledged', 'Verifier', 'finding:write'),
  ('finding', 'acknowledged', 'correcting', 'controller', 'finding:write'),
  ('finding', 'correcting', 'resolved', 'controller', 'finding:write'),
  ('finding', 'acknowledged', 'disputed', 'PM', 'finding:write'),
  ('finding', 'acknowledged', 'disputed', 'Developer', 'finding:write'),
  ('finding', 'acknowledged', 'escalated', 'controller', 'finding:write'),
  ('run_control', 'active', 'paused', 'operator', 'run:control'),
  ('run_control', 'paused', 'active', 'operator', 'run:control'),
  ('run_control', 'active', 'canceling', 'operator', 'run:control'),
  ('run_control', 'paused', 'canceling', 'operator', 'run:control'),
  ('run_control', 'canceling', 'canceled', 'controller', 'run:control'),
  ('run_control', 'active', 'completed', 'controller', 'run:control'),
  ('run_control', 'active', 'failed', 'controller', 'run:control');

CREATE TRIGGER immutable_project_revisions_update BEFORE UPDATE ON project_revisions BEGIN SELECT RAISE(ABORT, 'project revisions are immutable'); END;
CREATE TRIGGER immutable_project_revisions_delete BEFORE DELETE ON project_revisions BEGIN SELECT RAISE(ABORT, 'project revisions are immutable'); END;
CREATE TRIGGER immutable_assignment_input_bindings_update BEFORE UPDATE ON assignment_input_bindings BEGIN SELECT RAISE(ABORT, 'assignment input bindings are immutable'); END;
CREATE TRIGGER immutable_assignment_input_bindings_delete BEFORE DELETE ON assignment_input_bindings BEGIN SELECT RAISE(ABORT, 'assignment input bindings are immutable'); END;
CREATE TRIGGER immutable_command_receipts_update BEFORE UPDATE ON command_receipts BEGIN SELECT RAISE(ABORT, 'command receipts are immutable'); END;
CREATE TRIGGER immutable_command_receipts_delete BEFORE DELETE ON command_receipts BEGIN SELECT RAISE(ABORT, 'command receipts are immutable'); END;
CREATE TRIGGER immutable_candidates_update BEFORE UPDATE ON candidates BEGIN SELECT RAISE(ABORT, 'candidates are immutable'); END;
CREATE TRIGGER immutable_candidates_delete BEFORE DELETE ON candidates BEGIN SELECT RAISE(ABORT, 'candidates are immutable'); END;
CREATE TRIGGER immutable_candidate_evidence_update BEFORE UPDATE ON candidate_evidence BEGIN SELECT RAISE(ABORT, 'candidate evidence is immutable'); END;
CREATE TRIGGER immutable_candidate_evidence_delete BEFORE DELETE ON candidate_evidence BEGIN SELECT RAISE(ABORT, 'candidate evidence is immutable'); END;
CREATE TRIGGER immutable_outbox_attempts_update BEFORE UPDATE ON outbox_delivery_attempts BEGIN SELECT RAISE(ABORT, 'delivery attempts are immutable'); END;
CREATE TRIGGER immutable_outbox_attempts_delete BEFORE DELETE ON outbox_delivery_attempts BEGIN SELECT RAISE(ABORT, 'delivery attempts are immutable'); END;
CREATE TRIGGER immutable_finding_responses_update BEFORE UPDATE ON finding_responses BEGIN SELECT RAISE(ABORT, 'finding responses are immutable'); END;
CREATE TRIGGER immutable_finding_responses_delete BEFORE DELETE ON finding_responses BEGIN SELECT RAISE(ABORT, 'finding responses are immutable'); END;
CREATE TRIGGER immutable_finding_dispositions_update BEFORE UPDATE ON finding_dispositions BEGIN SELECT RAISE(ABORT, 'finding dispositions are immutable'); END;
CREATE TRIGGER immutable_finding_dispositions_delete BEFORE DELETE ON finding_dispositions BEGIN SELECT RAISE(ABORT, 'finding dispositions are immutable'); END;
CREATE TRIGGER immutable_controller_events_update BEFORE UPDATE ON controller_events BEGIN SELECT RAISE(ABORT, 'controller events are immutable'); END;
CREATE TRIGGER immutable_controller_events_delete BEFORE DELETE ON controller_events BEGIN SELECT RAISE(ABORT, 'controller events are immutable'); END;
CREATE TRIGGER immutable_operator_actions_update BEFORE UPDATE ON operator_actions BEGIN SELECT RAISE(ABORT, 'operator actions are immutable'); END;
CREATE TRIGGER immutable_operator_actions_delete BEFORE DELETE ON operator_actions BEGIN SELECT RAISE(ABORT, 'operator actions are immutable'); END;
CREATE TRIGGER immutable_recovery_attempts_delete BEFORE DELETE ON recovery_attempts BEGIN SELECT RAISE(ABORT, 'recovery attempts are immutable'); END;
CREATE TRIGGER immutable_usage_observations_update BEFORE UPDATE ON usage_observations BEGIN SELECT RAISE(ABORT, 'usage observations are immutable'); END;
CREATE TRIGGER immutable_usage_observations_delete BEFORE DELETE ON usage_observations BEGIN SELECT RAISE(ABORT, 'usage observations are immutable'); END;
