ALTER TABLE assignments ADD COLUMN worker_actor_id TEXT REFERENCES actors(actor_id);


INSERT INTO transition_rules(entity_type, from_state, to_state, role, capability)
VALUES ('work_item', 'awaiting_verification', 'canceled', 'controller', 'work:assign');
