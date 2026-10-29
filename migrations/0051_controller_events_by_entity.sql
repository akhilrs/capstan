-- The feed reads the creation row of one entity (project, type, id) for the rows that point back at it. The existing
-- entity index leads with the state instead, so that read scanned every row of the type; this one answers it directly.
CREATE INDEX IF NOT EXISTS controller_events_by_entity ON controller_events(project_id, entity_type, entity_id, sequence);
