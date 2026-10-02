-- PM notices: one controller message to the PM per (kind, subject, episode), so a delivery problem or a stall is told once.
CREATE TABLE pm_notices (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  notice_id TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('delivery', 'stalled', 'blocked')),
  subject TEXT NOT NULL CHECK (trim(subject) <> ''),
  episode TEXT NOT NULL CHECK (trim(episode) <> ''),
  message_id TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, notice_id),
  UNIQUE (project_id, kind, subject, episode),
  FOREIGN KEY (project_id, message_id) REFERENCES messages(project_id, message_id)
) STRICT, WITHOUT ROWID;

-- Each wake line the controller types into an idle PM, recorded before it is typed.
CREATE TABLE pm_wakes (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  wake_id TEXT NOT NULL,
  message_id TEXT NOT NULL,
  sent_at TEXT NOT NULL,
  PRIMARY KEY (project_id, wake_id),
  FOREIGN KEY (project_id, message_id) REFERENCES messages(project_id, message_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX pm_wakes_by_message ON pm_wakes(project_id, message_id, sent_at);

-- The routine checks the controller queues for the Supervisor; they never produce a delivery notice.
CREATE TABLE supervision_checks (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  message_id TEXT NOT NULL,
  queued_at TEXT NOT NULL,
  PRIMARY KEY (project_id, message_id),
  FOREIGN KEY (project_id, message_id) REFERENCES messages(project_id, message_id)
) STRICT, WITHOUT ROWID;
