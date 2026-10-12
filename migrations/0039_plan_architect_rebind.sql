-- A plan's architect may be rebound to the active successor of a replaced architect (the same role) while the plan is not
-- superseded, so reports, review requests and sign-off follow the replacement. Every other change of the architect is still refused.
DROP TRIGGER plans_architect_is_final;
CREATE TRIGGER plans_architect_is_final BEFORE UPDATE OF architect_agent_id ON plans
  WHEN OLD.architect_agent_id IS NOT NULL AND NEW.architect_agent_id IS NOT OLD.architect_agent_id
    AND NOT (OLD.state <> 'superseded' AND EXISTS (
      SELECT 1 FROM agents successor JOIN agents predecessor ON predecessor.project_id = successor.project_id
      WHERE successor.project_id = OLD.project_id AND successor.agent_id = NEW.architect_agent_id AND successor.state = 'active'
        AND predecessor.agent_id = OLD.architect_agent_id AND predecessor.state <> 'active'
        AND predecessor.role_name = successor.role_name))
  BEGIN SELECT RAISE(ABORT, 'the plan architect is final'); END;
