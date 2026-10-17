CREATE TABLE approval_windows (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  window_id TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  kind TEXT NOT NULL CHECK (kind IN ('hour', 'session', 'minutes')),
  minutes INTEGER NOT NULL CHECK (minutes BETWEEN 1 AND 480),
  created_by TEXT NOT NULL,
  source_proposal_id TEXT,
  source_relay_id TEXT,
  created_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  operator_ended_at TEXT,
  operator_ended_reason TEXT CHECK (operator_ended_reason IS NULL OR operator_ended_reason = 'released'),
  ended_at TEXT,
  ended_reason TEXT CHECK (ended_reason IS NULL OR ended_reason IN ('expired', 'revoked', 'replaced', 'restart')),
  PRIMARY KEY (project_id, window_id),
  UNIQUE (project_id, sequence),
  FOREIGN KEY (project_id, created_by) REFERENCES actors(project_id, actor_id),
  FOREIGN KEY (project_id, source_proposal_id) REFERENCES operator_proposals(project_id, proposal_id),
  FOREIGN KEY (project_id, source_relay_id) REFERENCES prompt_relays(project_id, relay_id),
  CHECK ((source_proposal_id IS NULL) <> (source_relay_id IS NULL)),
  CHECK ((operator_ended_at IS NULL) = (operator_ended_reason IS NULL)),
  CHECK ((ended_at IS NULL) = (ended_reason IS NULL))
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX approval_windows_one_open ON approval_windows(project_id) WHERE ended_at IS NULL;

CREATE TRIGGER approval_windows_identity_is_immutable BEFORE UPDATE OF project_id, window_id, sequence, kind, minutes, created_by, source_proposal_id, source_relay_id, created_at, expires_at ON approval_windows
  BEGIN SELECT RAISE(ABORT, 'a window kind, length, source and expiry are immutable'); END;
CREATE TRIGGER approval_windows_operator_end_is_final BEFORE UPDATE OF operator_ended_at, operator_ended_reason ON approval_windows
  WHEN OLD.operator_ended_at IS NOT NULL
  BEGIN SELECT RAISE(ABORT, 'the operator part of a window ends once'); END;
CREATE TRIGGER approval_windows_end_is_final BEFORE UPDATE OF ended_at, ended_reason ON approval_windows
  WHEN OLD.ended_at IS NOT NULL
  BEGIN SELECT RAISE(ABORT, 'a window ends once'); END;
CREATE TRIGGER immutable_approval_windows_delete BEFORE DELETE ON approval_windows
  BEGIN SELECT RAISE(ABORT, 'approval windows are immutable'); END;

-- A timed grant of plan 40 does not survive: windows replace them.
UPDATE operator_grants SET ended_reason = 'restart' WHERE kind = 'timed' AND ended_reason IS NULL;

-- The controller answers a worker prompt only inside an open window. The window id is recorded with the capture and
-- with the answer; the answer's time is written by the code (never read from the SQL clock).
ALTER TABLE prompt_relays ADD COLUMN auto_window_id TEXT;
ALTER TABLE prompt_relays ADD COLUMN auto_begin_at TEXT;

DROP TRIGGER prompt_relays_capture_by_a_pm;
DROP TRIGGER prompt_relays_answer_by_a_pm;

CREATE TRIGGER prompt_relays_capture_by_a_pm_or_in_a_window BEFORE INSERT ON prompt_relays
  WHEN NOT (
    (NEW.auto_window_id IS NULL AND NEW.auto_begin_at IS NULL
      AND EXISTS (SELECT 1 FROM actors a WHERE a.project_id = NEW.project_id AND a.actor_id = NEW.captured_by_actor_id AND a.role = 'PM'))
    OR
    (NEW.auto_window_id IS NOT NULL AND NEW.auto_begin_at IS NULL
      AND EXISTS (SELECT 1 FROM actors a WHERE a.project_id = NEW.project_id AND a.actor_id = NEW.captured_by_actor_id AND a.role = 'controller' AND a.is_internal = 1 AND a.active = 1 AND a.revoked_at IS NULL)
      AND EXISTS (SELECT 1 FROM approval_windows w WHERE w.project_id = NEW.project_id AND w.window_id = NEW.auto_window_id AND w.ended_at IS NULL AND w.expires_at > NEW.captured_at)
      AND EXISTS (SELECT 1 FROM agents g WHERE g.project_id = NEW.project_id AND g.agent_id = NEW.agent_id AND g.kind <> 'PM'))
  )
  BEGIN SELECT RAISE(ABORT, 'a prompt is captured by a PM actor, or by the controller inside an open window for a non-PM agent'); END;
CREATE TRIGGER prompt_relays_answer_by_a_pm_or_in_a_window BEFORE UPDATE OF answered_by_actor_id ON prompt_relays
  WHEN NEW.answered_by_actor_id IS NOT NULL
   AND NOT (
     (NEW.auto_begin_at IS NULL
       AND EXISTS (SELECT 1 FROM actors a WHERE a.project_id = NEW.project_id AND a.actor_id = NEW.answered_by_actor_id AND a.role = 'PM' AND a.active = 1 AND a.revoked_at IS NULL))
     OR
     (NEW.auto_window_id IS NOT NULL AND NEW.auto_begin_at IS NOT NULL
       AND NEW.answer_kind = 'option'
       AND EXISTS (SELECT 1 FROM actors a WHERE a.project_id = NEW.project_id AND a.actor_id = NEW.answered_by_actor_id AND a.role = 'controller' AND a.is_internal = 1 AND a.active = 1 AND a.revoked_at IS NULL)
       AND EXISTS (SELECT 1 FROM approval_windows w WHERE w.project_id = NEW.project_id AND w.window_id = NEW.auto_window_id AND w.ended_at IS NULL AND w.expires_at > NEW.auto_begin_at)
       AND EXISTS (SELECT 1 FROM agents g WHERE g.project_id = NEW.project_id AND g.agent_id = NEW.agent_id AND g.kind <> 'PM')
       AND EXISTS (SELECT 1 FROM json_each(NEW.options_json) o
                   WHERE json_extract(o.value, '$.number') = NEW.answer_option
                     AND COALESCE(json_extract(o.value, '$.widensPermissions'), 0) = 0
                     AND COALESCE(json_extract(o.value, '$.acceptsText'), 0) = 0))
   )
  BEGIN SELECT RAISE(ABORT, 'a prompt is answered by an active PM actor, or by the controller inside an open window with an option that neither widens nor takes text'); END;
CREATE TRIGGER prompt_relays_auto_window_is_final BEFORE UPDATE OF auto_window_id, auto_begin_at ON prompt_relays
  WHEN OLD.auto_window_id IS NOT NULL AND (NEW.auto_window_id IS NOT OLD.auto_window_id OR (OLD.auto_begin_at IS NOT NULL AND NEW.auto_begin_at IS NOT OLD.auto_begin_at))
  BEGIN SELECT RAISE(ABORT, 'the window of a prompt relay is final'); END;

INSERT INTO role_capabilities(role, capability) VALUES ('controller', 'prompt:relay');

INSERT OR IGNORE INTO capability_grants(project_id, actor_id, capability, granted_by, granted_at)
SELECT a.project_id, a.actor_id, 'prompt:relay', a.actor_id, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM actors a
WHERE a.is_internal = 1 AND a.role = 'controller' AND a.active = 1 AND a.revoked_at IS NULL;
