-- Messages from an agent to the user (`cstan send @user`): the other half of the remote conversation with the PM. A message
-- is an `action` (it asks the user something and waits in the remote clients' waiting list until answered) or `info` (it
-- tells the user something). Kept as the conversation history; the snapshot lists the latest ones.
CREATE TABLE IF NOT EXISTS user_messages (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  message_id TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  sender_agent_id TEXT NOT NULL,
  sender_actor_id TEXT NOT NULL,
  class TEXT NOT NULL CHECK (class IN ('action', 'info')),
  body TEXT NOT NULL,
  body_hash TEXT NOT NULL CHECK (length(body_hash) = 64),
  state TEXT NOT NULL CHECK (state IN ('open', 'answered', 'info')),
  reply_message_id TEXT,
  created_at TEXT NOT NULL,
  answered_at TEXT,
  PRIMARY KEY (project_id, message_id),
  UNIQUE (project_id, sequence),
  FOREIGN KEY (project_id, sender_agent_id) REFERENCES agents(project_id, agent_id),
  FOREIGN KEY (project_id, sender_actor_id) REFERENCES actors(project_id, actor_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS user_messages_by_state ON user_messages(project_id, state, sequence);
