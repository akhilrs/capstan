CREATE TABLE role_definitions (
  project_id TEXT NOT NULL REFERENCES projects(project_id),
  role_name TEXT NOT NULL CHECK (role_name GLOB '[a-z]*' AND length(role_name) BETWEEN 1 AND 32 AND role_name NOT GLOB '*[^a-z0-9-]*'),
  kind TEXT NOT NULL CHECK (kind IN ('PM', 'Developer', 'Verifier', 'Supervisor')),
  host TEXT NOT NULL CHECK (length(host) BETWEEN 1 AND 32),
  config_hash TEXT NOT NULL CHECK (length(config_hash) = 64),
  state TEXT NOT NULL CHECK (state IN ('active', 'retired')),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (project_id, role_name)
) STRICT, WITHOUT ROWID;
