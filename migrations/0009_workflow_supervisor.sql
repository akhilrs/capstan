ALTER TABLE findings ADD COLUMN affected_seat_id TEXT;
ALTER TABLE findings ADD COLUMN affected_work_item_id TEXT;
ALTER TABLE findings ADD COLUMN affected_assignment_id TEXT;
ALTER TABLE findings ADD COLUMN affected_generation INTEGER CHECK (affected_generation IS NULL OR affected_generation > 0);
ALTER TABLE findings ADD COLUMN detected_after_sequence INTEGER NOT NULL DEFAULT 0 CHECK (detected_after_sequence >= 0);
ALTER TABLE findings ADD COLUMN acknowledgement_deadline TEXT;
ALTER TABLE findings ADD COLUMN escalation_route TEXT NOT NULL DEFAULT 'operator';
ALTER TABLE findings ADD COLUMN intervention_count INTEGER NOT NULL DEFAULT 0 CHECK (intervention_count >= 0);
ALTER TABLE findings ADD COLUMN cooldown_until TEXT;
ALTER TABLE findings ADD COLUMN reopened_from_finding_id TEXT;
ALTER TABLE finding_deliveries RENAME TO finding_deliveries_v8;

CREATE TABLE finding_deliveries (
  project_id TEXT NOT NULL,
  finding_id TEXT NOT NULL,
  delivery_id TEXT NOT NULL,
  seat_id TEXT NOT NULL,
  command_id TEXT NOT NULL,
  assignment_id TEXT,
  generation INTEGER CHECK (generation IS NULL OR generation > 0),
  delivered_at TEXT,
  acknowledged_at TEXT,
  PRIMARY KEY (project_id, delivery_id),
  FOREIGN KEY (project_id, finding_id) REFERENCES findings(project_id, finding_id),
  FOREIGN KEY (project_id, seat_id) REFERENCES seats(project_id, seat_id),
  FOREIGN KEY (project_id, command_id) REFERENCES commands(project_id, command_id),
  FOREIGN KEY (project_id, assignment_id) REFERENCES assignments(project_id, assignment_id)
) STRICT, WITHOUT ROWID;

INSERT INTO finding_deliveries(project_id, finding_id, delivery_id, seat_id, command_id,
  delivered_at, acknowledged_at)
SELECT project_id, finding_id, delivery_id, seat_id, command_id, delivered_at, acknowledged_at
FROM finding_deliveries_v8;
DROP TABLE finding_deliveries_v8;

CREATE INDEX findings_dedup_lookup
  ON findings(project_id, affected_assignment_id, fingerprint, created_at DESC);

CREATE TABLE supervision_control (
  project_id TEXT PRIMARY KEY REFERENCES projects(project_id),
  enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
  bootstrap_pm_allowed INTEGER NOT NULL DEFAULT 0 CHECK (bootstrap_pm_allowed IN (0, 1)),
  health TEXT NOT NULL CHECK (health IN ('healthy', 'evaluating', 'degraded')),
  target_epoch INTEGER NOT NULL CHECK (target_epoch >= 0),
  checkpoint_epoch INTEGER,
  checkpoint_assignment_id TEXT,
  checkpoint_event_sequence INTEGER,
  checkpoint_fingerprint TEXT,
  replacement_attempts INTEGER NOT NULL DEFAULT 0 CHECK (replacement_attempts BETWEEN 0 AND 1),
  updated_at TEXT NOT NULL,
  FOREIGN KEY (project_id, checkpoint_assignment_id) REFERENCES assignments(project_id, assignment_id)
) STRICT;

INSERT INTO supervision_control(project_id, enabled, health, target_epoch, checkpoint_epoch, updated_at)
SELECT project_id, 0, 'degraded', 0, NULL, strftime('%Y-%m-%dT%H:%M:%fZ','now')
FROM projects;

CREATE TABLE finding_resolution_evidence (
  project_id TEXT NOT NULL,
  evidence_id TEXT NOT NULL,
  finding_id TEXT NOT NULL,
  event_id TEXT NOT NULL,
  assignment_id TEXT NOT NULL,
  generation INTEGER NOT NULL CHECK (generation > 0),
  condition TEXT NOT NULL,
  evidence_json TEXT NOT NULL,
  created_by TEXT NOT NULL REFERENCES actors(actor_id),
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, evidence_id),
  UNIQUE (project_id, finding_id, event_id),
  FOREIGN KEY (project_id, finding_id) REFERENCES findings(project_id, finding_id),
  FOREIGN KEY (project_id, assignment_id) REFERENCES assignments(project_id, assignment_id)
 ) STRICT, WITHOUT ROWID;
CREATE TABLE finding_correction_work (
  project_id TEXT NOT NULL,
  work_item_id TEXT NOT NULL,
  finding_id TEXT NOT NULL,
  target_assignment_id TEXT NOT NULL,
  target_generation INTEGER NOT NULL CHECK (target_generation > 0),
  target_seat_id TEXT NOT NULL,
  target_role TEXT NOT NULL CHECK (target_role IN ('PM', 'Developer', 'Verifier')),
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, work_item_id),
  FOREIGN KEY (project_id, work_item_id) REFERENCES work_items(project_id, work_item_id),
  FOREIGN KEY (project_id, finding_id) REFERENCES findings(project_id, finding_id),
  FOREIGN KEY (project_id, target_assignment_id) REFERENCES assignments(project_id, assignment_id),
  FOREIGN KEY (project_id, target_seat_id) REFERENCES seats(project_id, seat_id)
) STRICT, WITHOUT ROWID;

CREATE TRIGGER immutable_finding_resolution_evidence_update
BEFORE UPDATE ON finding_resolution_evidence
BEGIN SELECT RAISE(ABORT, 'finding resolution evidence is immutable'); END;
CREATE TRIGGER immutable_finding_resolution_evidence_delete
BEFORE DELETE ON finding_resolution_evidence
BEGIN SELECT RAISE(ABORT, 'finding resolution evidence is immutable'); END;

INSERT INTO transition_rules(entity_type, from_state, to_state, role, capability) VALUES
  ('work_item', 'blocked', 'canceled', 'controller', 'work:assign');
INSERT INTO transition_rules(entity_type, from_state, to_state, role, capability) VALUES
  ('finding', 'correcting', 'escalated', 'controller', 'finding:write'),
  ('finding', 'disputed', 'escalated', 'controller', 'finding:write');
INSERT INTO transition_rules(entity_type, from_state, to_state, role, capability) VALUES
  ('finding', 'correcting', 'correcting', 'controller', 'finding:write');
INSERT INTO transition_rules(entity_type, from_state, to_state, role, capability) VALUES
  ('finding', 'acknowledged', 'disputed', 'Verifier', 'finding:write');
INSERT INTO transition_rules(entity_type, from_state, to_state, role, capability) VALUES
  ('finding', 'reported', 'escalated', 'controller', 'finding:write'),
  ('finding', 'reported', 'escalated', 'operator', 'finding:write');