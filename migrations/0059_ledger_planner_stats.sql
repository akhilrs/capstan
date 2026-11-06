-- Planner statistics and two indexes for the hot paths: the retention prune (oldest request rows of a project) and the
-- notice lookups that compare the head of a message body.
-- The ANALYZE comes last so the new indexes get statistics too.
PRAGMA analysis_limit = 1000;
CREATE INDEX mutation_requests_by_created ON mutation_requests(project_id, created_at);
CREATE INDEX messages_by_body_head ON messages(project_id, substr(body, 1, 120));
ANALYZE;
