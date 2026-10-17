-- A restart proposal may start approved by an approval window (auto_rule 'window:<id>'), besides full auto.
DROP TRIGGER operator_proposals_start_undecided;
CREATE TRIGGER operator_proposals_start_undecided BEFORE INSERT ON operator_proposals
  WHEN NOT (NEW.state = 'proposed' AND NEW.auto_rule IS NULL AND NEW.decided_at IS NULL AND NEW.decided_by_actor_id IS NULL)
   AND NOT (NEW.state = 'approved' AND NEW.auto_rule IS NOT NULL AND NEW.decided_at IS NOT NULL AND NEW.decided_by_actor_id IS NULL
            AND (NEW.kind = 'command' OR NEW.auto_rule = 'full-auto' OR NEW.auto_rule LIKE 'window:%'))
  BEGIN SELECT RAISE(ABORT, 'a proposal starts as proposed, or as approved by an auto-approve rule'); END;
