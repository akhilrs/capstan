INSERT INTO transition_rules(entity_type, from_state, to_state, role, capability) VALUES
  ('command', 'queued', 'unknown', 'controller', 'controller:reconcile'),
  ('runtime_session', 'stopping', 'unknown', 'controller', 'controller:reconcile');
