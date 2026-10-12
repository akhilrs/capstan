-- A plan's review rounds are limited per plan (NULL on plans opened before this migration: the default limit applies), a
-- review of a plan that ends in findings is classified, and a revision may be an amendment of the previous one: a narrow
-- edit the architect makes after a review whose findings were all mechanical, without another review round.
ALTER TABLE plans ADD COLUMN max_review_rounds INTEGER CHECK (max_review_rounds IS NULL OR max_review_rounds BETWEEN 1 AND 20);

ALTER TABLE plan_revisions ADD COLUMN amendment_json TEXT CHECK (amendment_json IS NULL OR (json_valid(amendment_json) AND json_type(amendment_json) = 'object'));
ALTER TABLE plan_revisions ADD COLUMN amends_review_id TEXT;

ALTER TABLE reviews ADD COLUMN findings_class TEXT CHECK (findings_class IS NULL OR findings_class IN ('design', 'mechanical'));

CREATE TRIGGER plan_revisions_amendment_is_a_pair BEFORE INSERT ON plan_revisions
  WHEN (NEW.amendment_json IS NULL) <> (NEW.amends_review_id IS NULL)
  BEGIN SELECT RAISE(ABORT, 'an amendment and the review it answers are set together'); END;
CREATE TRIGGER plan_revisions_amend_a_mechanical_review BEFORE INSERT ON plan_revisions
  WHEN NEW.amends_review_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM reviews r
    WHERE r.project_id = NEW.project_id AND r.review_id = NEW.amends_review_id AND r.subject_plan_id = NEW.plan_id
      AND r.subject_plan_revision = NEW.revision - 1 AND r.state = 'findings' AND r.findings_class = 'mechanical')
  BEGIN SELECT RAISE(ABORT, 'an amendment answers a finished mechanical findings review of the previous revision'); END;
CREATE TRIGGER plan_revisions_amend_a_review_once BEFORE INSERT ON plan_revisions
  WHEN NEW.amends_review_id IS NOT NULL AND EXISTS (
    SELECT 1 FROM plan_revisions o WHERE o.project_id = NEW.project_id AND o.amends_review_id = NEW.amends_review_id)
  BEGIN SELECT RAISE(ABORT, 'a review is amended only once'); END;

CREATE TRIGGER reviews_findings_class_only_for_plan_findings BEFORE INSERT ON reviews
  WHEN NEW.findings_class IS NOT NULL
  BEGIN SELECT RAISE(ABORT, 'a findings class is set when a plan review finishes'); END;
CREATE TRIGGER reviews_findings_class_on_finish BEFORE UPDATE OF findings_class ON reviews
  WHEN NEW.findings_class IS NOT NULL AND (NEW.subject_plan_id IS NULL OR NEW.state <> 'findings')
  BEGIN SELECT RAISE(ABORT, 'only a plan review that finished with findings has a findings class'); END;

DROP TRIGGER finished_reviews_are_final;
CREATE TRIGGER finished_reviews_are_final BEFORE UPDATE OF state, verdict_text, failure_reason, completed_at, findings_class ON reviews
  WHEN OLD.state <> 'started' BEGIN SELECT RAISE(ABORT, 'a finished review is final'); END;

CREATE TRIGGER plans_review_limit_is_immutable BEFORE UPDATE OF max_review_rounds ON plans
  BEGIN SELECT RAISE(ABORT, 'the plan review limit is immutable'); END;
