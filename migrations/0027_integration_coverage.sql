CREATE TABLE integration_covered_reports (
  project_id TEXT NOT NULL,
  integration_id TEXT NOT NULL,
  report_id TEXT NOT NULL,
  how TEXT NOT NULL CHECK (how IN ('ancestor', 'tree')),
  PRIMARY KEY (project_id, integration_id, report_id),
  FOREIGN KEY (project_id, integration_id) REFERENCES integrations(project_id, integration_id),
  FOREIGN KEY (project_id, report_id) REFERENCES agent_reports(project_id, report_id)
) STRICT, WITHOUT ROWID;

CREATE INDEX integration_covered_reports_by_report ON integration_covered_reports(project_id, report_id);

CREATE TRIGGER integration_covered_reports_only_confirmed BEFORE INSERT ON integration_covered_reports
  WHEN (SELECT state FROM integrations WHERE project_id = NEW.project_id AND integration_id = NEW.integration_id) IS NOT 'confirmed'
  BEGIN SELECT RAISE(ABORT, 'only a confirmed integration covers reports'); END;
CREATE TRIGGER integration_covered_reports_immutable BEFORE UPDATE ON integration_covered_reports
  BEGIN SELECT RAISE(ABORT, 'coverage rows are immutable'); END;
