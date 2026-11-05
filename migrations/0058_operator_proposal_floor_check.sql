-- What the approval floor said about a proposal when it was made: clear, unchecked or floor (with the entry that holds it
-- back). Only the Operator service writes them; a proposal made without a check, and every row from before, has NULL in
-- both and a remote client then offers it only an approve-once.
ALTER TABLE operator_proposals ADD COLUMN floor_check TEXT CHECK (floor_check IS NULL OR floor_check IN ('clear', 'unchecked', 'floor'));
ALTER TABLE operator_proposals ADD COLUMN floor_entry TEXT CHECK (floor_entry IS NULL OR (floor_check = 'floor' AND length(CAST(floor_entry AS BLOB)) <= 1024));

-- A proposal is also approved by the Operator role when the user chose the approval in a remote client (the kernel lets the
-- operator role approve only through that path; `cstan op decide` by the operator is still refused there).
DROP TRIGGER operator_proposals_approval_needs_a_pm;
CREATE TRIGGER operator_proposals_approval_needs_a_pm BEFORE UPDATE OF state ON operator_proposals
  WHEN NEW.state = 'approved'
   AND NOT (NEW.decided_at IS NOT NULL
        AND (NEW.auto_rule IS NOT NULL
          OR (NEW.decided_by_actor_id IS NOT NULL
              AND NEW.decided_by_actor_id <> NEW.proposer_actor_id
              AND (EXISTS (SELECT 1 FROM agents a WHERE a.project_id = NEW.project_id AND a.actor_id = NEW.decided_by_actor_id AND a.kind = 'PM')
                OR EXISTS (SELECT 1 FROM actors o WHERE o.project_id = NEW.project_id AND o.actor_id = NEW.decided_by_actor_id AND o.role = 'operator')))))
  BEGIN SELECT RAISE(ABORT, 'a proposal is approved by a PM agent or the operator (from a remote client) other than its proposer, or by an auto-approve rule'); END;
