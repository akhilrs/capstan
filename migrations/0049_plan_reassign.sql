-- A plan whose architect is lost can be handed to another active architect of the same or another role by the PM or the
-- operator. Each handover is one append-only row; the ledger lets the plan's architect move only to the target of the
-- newest row of that plan, and only while the plan is live and the old owner has stopped.
CREATE TABLE IF NOT EXISTS plan_reassignments (
  project_id TEXT NOT NULL,
  plan_id TEXT NOT NULL,
  sequence INTEGER NOT NULL CHECK (sequence > 0),
  from_agent_id TEXT NOT NULL,
  to_agent_id TEXT NOT NULL CHECK (to_agent_id <> from_agent_id),
  actor_id TEXT NOT NULL,
  reason TEXT,
  created_at TEXT NOT NULL,
  PRIMARY KEY (project_id, plan_id, sequence),
  FOREIGN KEY (project_id, plan_id) REFERENCES plans(project_id, plan_id),
  FOREIGN KEY (project_id, from_agent_id) REFERENCES agents(project_id, agent_id),
  FOREIGN KEY (project_id, to_agent_id) REFERENCES agents(project_id, agent_id),
  FOREIGN KEY (project_id, actor_id) REFERENCES actors(project_id, actor_id)
) STRICT, WITHOUT ROWID;

CREATE TRIGGER IF NOT EXISTS immutable_plan_reassignments_update BEFORE UPDATE ON plan_reassignments
  BEGIN SELECT RAISE(ABORT, 'plan reassignments are immutable'); END;
CREATE TRIGGER IF NOT EXISTS immutable_plan_reassignments_delete BEFORE DELETE ON plan_reassignments
  BEGIN SELECT RAISE(ABORT, 'plan reassignments are immutable'); END;

DROP TRIGGER plans_architect_is_final;
CREATE TRIGGER plans_architect_is_final BEFORE UPDATE OF architect_agent_id ON plans
  WHEN OLD.architect_agent_id IS NOT NULL AND NEW.architect_agent_id IS NOT OLD.architect_agent_id
    AND NOT (OLD.state <> 'superseded' AND EXISTS (
      SELECT 1 FROM agents successor JOIN agents predecessor ON predecessor.project_id = successor.project_id
      WHERE successor.project_id = OLD.project_id AND successor.agent_id = NEW.architect_agent_id AND successor.state = 'active'
        AND predecessor.agent_id = OLD.architect_agent_id AND predecessor.state <> 'active'
        AND predecessor.role_name = successor.role_name))
    AND NOT (OLD.state <> 'superseded' AND OLD.cancelled_at IS NULL AND EXISTS (
      SELECT 1 FROM plan_reassignments r
        JOIN agents target ON target.project_id = r.project_id AND target.agent_id = r.to_agent_id
        JOIN agents owner ON owner.project_id = r.project_id AND owner.agent_id = r.from_agent_id
      WHERE r.project_id = OLD.project_id AND r.plan_id = OLD.plan_id
        AND r.sequence = (SELECT MAX(n.sequence) FROM plan_reassignments n WHERE n.project_id = OLD.project_id AND n.plan_id = OLD.plan_id)
        AND r.from_agent_id = OLD.architect_agent_id AND r.to_agent_id = NEW.architect_agent_id
        AND target.state = 'active' AND owner.state <> 'active'))
  BEGIN SELECT RAISE(ABORT, 'the plan architect is final'); END;
