ALTER TABLE commands
ADD COLUMN start_requested INTEGER NOT NULL DEFAULT 0 CHECK (start_requested IN (0, 1));