INSERT INTO transition_rules(entity_type, from_state, to_state, role, capability) VALUES
  ('finding', 'detected', 'escalated', 'controller', 'finding:write');
