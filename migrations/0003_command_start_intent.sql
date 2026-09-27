ALTER TABLE commands
ADD COLUMN start_requested INTEGER NOT NULL DEFAULT 0 CHECK (start_requested IN (0, 1));

-- A durable working/completed command from an older schema proves M1 was started.
UPDATE commands SET start_requested = 1 WHERE state IN ('started', 'completed');