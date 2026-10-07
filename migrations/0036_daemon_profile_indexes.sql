CREATE INDEX IF NOT EXISTS messages_by_state ON messages(project_id, state, recipient_agent_id, sequence);
CREATE INDEX IF NOT EXISTS agent_reports_by_agent_state ON agent_reports(project_id, agent_id, state, sequence);
CREATE INDEX IF NOT EXISTS reviews_by_subject_report ON reviews(project_id, subject_report_id, sequence);
CREATE INDEX IF NOT EXISTS controller_events_by_entity_state ON controller_events(project_id, entity_type, to_state, sequence);
