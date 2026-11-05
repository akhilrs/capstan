-- Episodes of a stuck PM: one row per episode, open until it recovers. The controller opens one when the PM has waited
-- too long (unread mail, no progress, a prompt) and closes it after the condition has been clear for 60 seconds, or when the
-- PM is paused, replaced or ended. At most one episode is open per agent. `alert` is false when the same agent id had an
-- alerting episode in the last 30 minutes; the rows are never pruned (the suppression reads them).
CREATE TABLE IF NOT EXISTS pm_stuck_episodes (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  episode_id TEXT NOT NULL CHECK (trim(episode_id) <> ''),
  agent_id TEXT NOT NULL,
  reason TEXT NOT NULL CHECK (reason IN ('unread_mail', 'no_progress', 'prompt')),
  since_ms INTEGER NOT NULL,
  minutes INTEGER NOT NULL CHECK (minutes > 0),
  alert INTEGER NOT NULL CHECK (alert IN (0, 1)),
  opened_ms INTEGER NOT NULL,
  clear_since_ms INTEGER,
  closed_at TEXT,
  closed_ms INTEGER,
  close_reason TEXT CHECK (close_reason IS NULL OR close_reason IN ('recovered', 'paused', 'replaced', 'ended')),
  PRIMARY KEY (project_id, episode_id),
  FOREIGN KEY (project_id, agent_id) REFERENCES agents(project_id, agent_id),
  CHECK ((closed_at IS NULL) = (close_reason IS NULL)),
  CHECK ((closed_at IS NULL) = (closed_ms IS NULL))
) STRICT, WITHOUT ROWID;

CREATE UNIQUE INDEX IF NOT EXISTS pm_stuck_episodes_one_open_per_agent ON pm_stuck_episodes(project_id, agent_id) WHERE closed_at IS NULL;
CREATE INDEX IF NOT EXISTS pm_stuck_episodes_by_alert ON pm_stuck_episodes(project_id, agent_id, opened_ms) WHERE alert = 1;
