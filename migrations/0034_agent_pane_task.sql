-- The task a worker was spawned for (--task) and its spawn title (--title); null when it was spawned without them.
ALTER TABLE agent_panes ADD COLUMN task_ref TEXT CHECK (task_ref IS NULL OR (length(task_ref) BETWEEN 1 AND 257 AND trim(task_ref) <> ''));
ALTER TABLE agent_panes ADD COLUMN task_title TEXT CHECK (task_title IS NULL OR (length(task_title) BETWEEN 1 AND 200 AND trim(task_title) <> ''));
