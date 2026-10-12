-- A message is either an action message (it asks the recipient for a step or must wake it) or an info message (it only
-- informs and may be delivered as a digest). Existing messages stay action messages. An info message never asks for an action.
ALTER TABLE messages ADD COLUMN class TEXT NOT NULL DEFAULT 'action' CHECK (class IN ('action', 'info'));
CREATE TRIGGER messages_info_needs_no_action_insert BEFORE INSERT ON messages
  WHEN NEW.class = 'info' AND NEW.action_needed = 1
  BEGIN SELECT RAISE(ABORT, 'an info message cannot need an action'); END;
CREATE TRIGGER messages_info_needs_no_action_update BEFORE UPDATE OF class, action_needed ON messages
  WHEN NEW.class = 'info' AND NEW.action_needed = 1
  BEGIN SELECT RAISE(ABORT, 'an info message cannot need an action'); END;
