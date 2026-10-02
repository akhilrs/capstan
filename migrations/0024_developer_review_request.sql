-- The designated architect is a Developer-kind agent that requests reviews of the reports it receives. The command layer
-- allows only the PM, the operator and the architect; this grant lets the ledger accept the architect's review request.
INSERT INTO role_capabilities(role, capability) VALUES ('Developer', 'review:request');

INSERT OR IGNORE INTO capability_grants(project_id, actor_id, capability, granted_by, granted_at)
SELECT a.project_id, a.actor_id, rc.capability, internal.actor_id, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM actors a
JOIN role_capabilities rc ON rc.role = a.role AND rc.capability = 'review:request'
JOIN actors internal ON internal.project_id = a.project_id AND internal.is_internal = 1
  AND internal.role = 'controller' AND internal.active = 1 AND internal.revoked_at IS NULL
WHERE a.active = 1 AND a.revoked_at IS NULL AND a.role = 'Developer';
