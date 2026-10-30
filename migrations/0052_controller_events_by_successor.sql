-- The wanted state of a requirement link follows the agents a bound agent replaced: each step of that chain finds the
-- replacement events whose recorded successor is the agent in hand. Without an index the step read the payload of every
-- event of the project (about 0.1 s per link on a ledger of 170 000 events); this one holds only the replacement events,
-- keyed by the successor, so a step is a lookup. The statement names it (INDEXED BY) because the planner does not choose
-- an expression index for a join on a recursive table.
CREATE INDEX IF NOT EXISTS controller_events_by_successor ON controller_events(project_id, json_extract(payload_json, '$.details.successorId'))
  WHERE entity_type = 'agent' AND to_state = 'replaced';
