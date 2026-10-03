CREATE TABLE prompt_relays (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  relay_id TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  agent_id TEXT NOT NULL,
  pane_id TEXT NOT NULL CHECK (trim(pane_id) <> ''),
  host_kind TEXT NOT NULL CHECK (trim(host_kind) <> ''),
  prompt_text TEXT NOT NULL CHECK (length(CAST(prompt_text AS BLOB)) <= 8192),
  options_json TEXT NOT NULL CHECK (json_valid(options_json)),
  prompt_sha TEXT NOT NULL CHECK (length(prompt_sha) = 64 AND prompt_sha NOT GLOB '*[^0-9a-f]*'),
  captured_by_actor_id TEXT NOT NULL,
  captured_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('captured', 'typing', 'answered', 'refused', 'failed', 'expired')),
  answer_kind TEXT CHECK (answer_kind IS NULL OR answer_kind IN ('option', 'esc', 'text')),
  answer_option INTEGER CHECK (answer_option IS NULL OR answer_option > 0),
  answer_widens_permissions INTEGER CHECK (answer_widens_permissions IS NULL OR answer_widens_permissions IN (0, 1)),
  answer_text TEXT CHECK (answer_text IS NULL OR length(CAST(answer_text AS BLOB)) <= 1000),
  answered_by_actor_id TEXT,
  answered_at TEXT,
  outcome_reason TEXT CHECK (outcome_reason IS NULL OR length(CAST(outcome_reason AS BLOB)) <= 1024),
  keys_json TEXT CHECK (keys_json IS NULL OR json_valid(keys_json)),
  PRIMARY KEY (project_id, relay_id),
  UNIQUE (project_id, sequence),
  FOREIGN KEY (project_id, agent_id) REFERENCES agents(project_id, agent_id),
  FOREIGN KEY (project_id, captured_by_actor_id) REFERENCES actors(project_id, actor_id),
  FOREIGN KEY (project_id, answered_by_actor_id) REFERENCES actors(project_id, actor_id),
  CHECK ((answer_kind IS NULL) = (answered_by_actor_id IS NULL)),
  CHECK ((answer_kind IS NULL) = (answer_widens_permissions IS NULL)),
  CHECK (answer_kind IS NULL OR (answer_kind = 'option' AND answer_option IS NOT NULL AND answer_text IS NULL)
      OR (answer_kind = 'esc' AND answer_option IS NULL AND answer_text IS NULL AND answer_widens_permissions = 0)
      OR (answer_kind = 'text' AND answer_option IS NOT NULL AND answer_text IS NOT NULL)),
  CHECK (state NOT IN ('captured', 'expired') OR answer_kind IS NULL),
  CHECK (state NOT IN ('typing', 'answered', 'refused', 'failed') OR answer_kind IS NOT NULL),
  CHECK ((answered_at IS NOT NULL) = (state IN ('answered', 'refused', 'failed'))),
  CHECK (state NOT IN ('answered', 'refused', 'failed') OR keys_json IS NOT NULL),
  CHECK ((outcome_reason IS NOT NULL) OR state IN ('captured', 'typing', 'answered')),
  CHECK (outcome_reason IS NULL OR state NOT IN ('captured', 'typing'))
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX prompt_relays_one_open_per_agent ON prompt_relays(project_id, agent_id) WHERE state IN ('captured', 'typing');
CREATE INDEX prompt_relays_by_state ON prompt_relays(project_id, state);

CREATE TRIGGER prompt_relays_start_captured BEFORE INSERT ON prompt_relays
  WHEN NOT (NEW.state = 'captured' AND NEW.answer_kind IS NULL AND NEW.answered_at IS NULL AND NEW.outcome_reason IS NULL AND NEW.keys_json IS NULL)
  BEGIN SELECT RAISE(ABORT, 'a prompt relay starts captured and unanswered'); END;
CREATE TRIGGER prompt_relays_capture_by_a_pm BEFORE INSERT ON prompt_relays
  WHEN NOT EXISTS (SELECT 1 FROM actors a WHERE a.project_id = NEW.project_id AND a.actor_id = NEW.captured_by_actor_id AND a.role = 'PM')
  BEGIN SELECT RAISE(ABORT, 'a prompt is captured by a PM actor'); END;
CREATE TRIGGER prompt_relays_identity_is_immutable BEFORE UPDATE OF project_id, relay_id, sequence, agent_id, pane_id, host_kind, prompt_text, options_json, prompt_sha, captured_by_actor_id, captured_at, expires_at ON prompt_relays
  BEGIN SELECT RAISE(ABORT, 'a captured prompt, its hash and its capture are immutable'); END;
CREATE TRIGGER prompt_relays_move_forward BEFORE UPDATE OF state ON prompt_relays
  WHEN NOT ((OLD.state = 'captured' AND NEW.state IN ('typing', 'refused', 'expired'))
         OR (OLD.state = 'typing' AND NEW.state IN ('answered', 'refused', 'failed')))
  BEGIN SELECT RAISE(ABORT, 'a prompt relay state only moves forward'); END;
CREATE TRIGGER prompt_relays_answer_is_final BEFORE UPDATE OF answer_kind, answer_option, answer_widens_permissions, answer_text, answered_by_actor_id ON prompt_relays
  WHEN OLD.answer_kind IS NOT NULL
  BEGIN SELECT RAISE(ABORT, 'a recorded answer is final'); END;
CREATE TRIGGER prompt_relays_outcome_is_final BEFORE UPDATE OF answered_at, outcome_reason, keys_json ON prompt_relays
  WHEN OLD.state IN ('answered', 'refused', 'failed', 'expired')
  BEGIN SELECT RAISE(ABORT, 'a finished prompt relay is final'); END;
CREATE TRIGGER prompt_relays_answer_by_a_pm BEFORE UPDATE OF answered_by_actor_id ON prompt_relays
  WHEN NEW.answered_by_actor_id IS NOT NULL
   AND NOT EXISTS (SELECT 1 FROM actors a WHERE a.project_id = NEW.project_id AND a.actor_id = NEW.answered_by_actor_id AND a.role = 'PM' AND a.active = 1 AND a.revoked_at IS NULL)
  BEGIN SELECT RAISE(ABORT, 'a prompt is answered by an active PM actor'); END;
CREATE TRIGGER prompt_relays_no_delete BEFORE DELETE ON prompt_relays
  BEGIN SELECT RAISE(ABORT, 'prompt relays are immutable'); END;

INSERT INTO role_capabilities(role, capability) VALUES ('PM', 'prompt:relay');

INSERT OR IGNORE INTO capability_grants(project_id, actor_id, capability, granted_by, granted_at)
SELECT a.project_id, a.actor_id, 'prompt:relay', internal.actor_id, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM actors a
JOIN actors internal ON internal.project_id = a.project_id AND internal.is_internal = 1
  AND internal.role = 'controller' AND internal.active = 1 AND internal.revoked_at IS NULL
WHERE a.active = 1 AND a.revoked_at IS NULL AND a.role = 'PM';
