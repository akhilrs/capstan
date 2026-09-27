ALTER TABLE assignments ADD COLUMN worker_actor_id TEXT REFERENCES actors(actor_id);

UPDATE assignments
SET worker_actor_id = (
  SELECT a.actor_id FROM actors a
  WHERE a.project_id = assignments.project_id
    AND a.seat_id = assignments.seat_id
  ORDER BY a.active DESC, a.actor_id
  LIMIT 1
)
WHERE worker_actor_id IS NULL;

INSERT INTO transition_rules(entity_type, from_state, to_state, role, capability)
VALUES ('work_item', 'awaiting_verification', 'canceled', 'controller', 'work:assign');
