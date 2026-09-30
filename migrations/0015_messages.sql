CREATE TABLE agents (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  agent_id TEXT NOT NULL,
  role_name TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('PM', 'Developer', 'Verifier', 'Supervisor')),
  seat_id TEXT NOT NULL,
  actor_id TEXT NOT NULL,
  generation INTEGER NOT NULL CHECK (generation >= 1),
  state TEXT NOT NULL CHECK (state IN ('active', 'ended')),
  last_activity_at TEXT NOT NULL,
  created_at TEXT NOT NULL,
  ended_at TEXT,
  PRIMARY KEY (project_id, agent_id),
  UNIQUE (project_id, actor_id),
  FOREIGN KEY (project_id, role_name) REFERENCES role_definitions(project_id, role_name),
  FOREIGN KEY (project_id, seat_id) REFERENCES seats(project_id, seat_id),
  FOREIGN KEY (project_id, actor_id) REFERENCES actors(project_id, actor_id)
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX one_active_agent_per_seat
  ON agents(project_id, seat_id)
  WHERE state = 'active';

CREATE TABLE agent_state_history (
  project_id TEXT NOT NULL,
  agent_id TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  herdr_state TEXT NOT NULL CHECK (herdr_state IN ('idle', 'working', 'blocked', 'done', 'unknown')),
  observed_at TEXT NOT NULL,
  PRIMARY KEY (project_id, agent_id, sequence),
  FOREIGN KEY (project_id, agent_id) REFERENCES agents(project_id, agent_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE agent_waits (
  project_id TEXT NOT NULL,
  wait_id TEXT NOT NULL,
  agent_id TEXT NOT NULL,
  started_at TEXT NOT NULL,
  ended_at TEXT,
  PRIMARY KEY (project_id, wait_id),
  FOREIGN KEY (project_id, agent_id) REFERENCES agents(project_id, agent_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX agent_waits_by_agent ON agent_waits(project_id, agent_id, started_at);

CREATE TABLE messages (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  message_id TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  recipient_agent_id TEXT NOT NULL,
  recipient_generation INTEGER NOT NULL CHECK (recipient_generation >= 1),
  sender_actor_id TEXT NOT NULL,
  body TEXT NOT NULL,
  body_hash TEXT NOT NULL CHECK (length(body_hash) = 64),
  state TEXT NOT NULL CHECK (state IN ('queued', 'deferred', 'sent', 'acked', 'acked_late', 'unacked', 'expired', 'cancelled', 'failed')),
  state_version INTEGER NOT NULL CHECK (state_version >= 0),
  queued_at TEXT NOT NULL,
  deferred_at TEXT,
  deferred_reason TEXT CHECK (deferred_reason IN ('agent_busy', 'agent_blocked', 'input_not_empty')),
  sent_at TEXT,
  acked_at TEXT,
  send_attempts INTEGER NOT NULL CHECK (send_attempts >= 0),
  cancel_reason TEXT,
  notified_at TEXT,
  last_notified_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (project_id, message_id),
  UNIQUE (project_id, sequence),
  FOREIGN KEY (project_id, recipient_agent_id) REFERENCES agents(project_id, agent_id),
  FOREIGN KEY (project_id, sender_actor_id) REFERENCES actors(project_id, actor_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX messages_by_recipient ON messages(project_id, recipient_agent_id, sequence);

CREATE TABLE rounds (
  project_id TEXT NOT NULL,
  round_id TEXT NOT NULL,
  assignment_id TEXT NOT NULL,
  ordinal INTEGER NOT NULL CHECK (ordinal > 0),
  instruction_message_id TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('open', 'closed')),
  opened_at TEXT NOT NULL,
  closed_at TEXT,
  PRIMARY KEY (project_id, round_id),
  UNIQUE (project_id, assignment_id, ordinal),
  FOREIGN KEY (project_id, assignment_id) REFERENCES assignments(project_id, assignment_id),
  FOREIGN KEY (project_id, instruction_message_id) REFERENCES messages(project_id, message_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE message_resolutions (
  project_id TEXT NOT NULL,
  resolution_id TEXT NOT NULL,
  message_id TEXT NOT NULL,
  decision TEXT NOT NULL CHECK (decision IN ('retry', 'skip', 'cancel')),
  decided_by TEXT NOT NULL,
  note TEXT,
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, resolution_id),
  FOREIGN KEY (project_id, message_id) REFERENCES messages(project_id, message_id),
  FOREIGN KEY (project_id, decided_by) REFERENCES actors(project_id, actor_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE message_input_clears (
  project_id TEXT NOT NULL,
  clear_id TEXT NOT NULL,
  message_id TEXT NOT NULL,
  text TEXT NOT NULL,
  text_hash TEXT NOT NULL CHECK (length(text_hash) = 64),
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, clear_id),
  FOREIGN KEY (project_id, message_id) REFERENCES messages(project_id, message_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE message_rejections (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  rejection_id TEXT NOT NULL,
  message_id TEXT,
  action TEXT NOT NULL,
  code TEXT NOT NULL,
  from_state TEXT,
  attempted_state TEXT,
  actor_id TEXT NOT NULL,
  reason TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, rejection_id),
  FOREIGN KEY (project_id, actor_id) REFERENCES actors(project_id, actor_id)
) STRICT, WITHOUT ROWID;

CREATE TRIGGER immutable_message_resolutions_update BEFORE UPDATE ON message_resolutions BEGIN SELECT RAISE(ABORT, 'message resolutions are immutable'); END;
CREATE TRIGGER immutable_message_resolutions_delete BEFORE DELETE ON message_resolutions BEGIN SELECT RAISE(ABORT, 'message resolutions are immutable'); END;
CREATE TRIGGER immutable_message_input_clears_update BEFORE UPDATE ON message_input_clears BEGIN SELECT RAISE(ABORT, 'message input clears are immutable'); END;
CREATE TRIGGER immutable_message_input_clears_delete BEFORE DELETE ON message_input_clears BEGIN SELECT RAISE(ABORT, 'message input clears are immutable'); END;
CREATE TRIGGER immutable_message_rejections_update BEFORE UPDATE ON message_rejections BEGIN SELECT RAISE(ABORT, 'message rejections are immutable'); END;
CREATE TRIGGER immutable_message_rejections_delete BEFORE DELETE ON message_rejections BEGIN SELECT RAISE(ABORT, 'message rejections are immutable'); END;

INSERT INTO role_capabilities(role, capability) VALUES
  ('operator', 'message:send'), ('PM', 'message:send'), ('Developer', 'message:send'),
  ('Verifier', 'message:send'), ('Supervisor', 'message:send'),
  ('PM', 'message:receive'), ('Developer', 'message:receive'),
  ('Verifier', 'message:receive'), ('Supervisor', 'message:receive'),
  ('operator', 'message:resolve'), ('PM', 'message:resolve');

INSERT INTO capability_grants(project_id, actor_id, capability, granted_by, granted_at)
SELECT a.project_id, a.actor_id, rc.capability, internal.actor_id, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM actors a
JOIN role_capabilities rc ON rc.role = a.role AND rc.capability IN ('message:send', 'message:receive', 'message:resolve')
JOIN actors internal ON internal.project_id = a.project_id AND internal.is_internal = 1
  AND internal.role = 'controller' AND internal.active = 1 AND internal.revoked_at IS NULL
WHERE a.active = 1 AND a.revoked_at IS NULL;
