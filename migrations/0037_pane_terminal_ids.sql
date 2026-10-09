-- The Herdr terminal id of a recorded pane. Herdr reuses short pane ids, so a pane is closed only while its terminal id still matches; null on rows recorded before this column.
ALTER TABLE agent_panes ADD COLUMN terminal_id TEXT CHECK (terminal_id IS NULL OR (length(terminal_id) BETWEEN 1 AND 128 AND terminal_id NOT GLOB '*[^A-Za-z0-9._:-]*'));
ALTER TABLE orphan_panes ADD COLUMN terminal_id TEXT CHECK (terminal_id IS NULL OR (length(terminal_id) BETWEEN 1 AND 128 AND terminal_id NOT GLOB '*[^A-Za-z0-9._:-]*'));
