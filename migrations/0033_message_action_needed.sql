-- A message can ask the PM for a step (a decision, a merge, a resolve), not only inform it.
-- Existing messages stay informational.
ALTER TABLE messages ADD COLUMN action_needed INTEGER NOT NULL DEFAULT 0 CHECK (action_needed IN (0, 1));
