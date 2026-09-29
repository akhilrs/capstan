UPDATE findings
SET affected_assignment_id = (
      SELECT MIN(c.assignment_id)
      FROM finding_deliveries d
      JOIN commands c ON c.project_id = d.project_id AND c.command_id = d.command_id
      JOIN assignments a ON a.project_id = c.project_id AND a.assignment_id = c.assignment_id
      WHERE d.project_id = findings.project_id AND d.finding_id = findings.finding_id
        AND d.seat_id = a.seat_id
      HAVING COUNT(DISTINCT c.assignment_id) = 1
    ),
    affected_seat_id = (
      SELECT MIN(a.seat_id)
      FROM finding_deliveries d
      JOIN commands c ON c.project_id = d.project_id AND c.command_id = d.command_id
      JOIN assignments a ON a.project_id = c.project_id AND a.assignment_id = c.assignment_id
      WHERE d.project_id = findings.project_id AND d.finding_id = findings.finding_id
        AND d.seat_id = a.seat_id
      HAVING COUNT(DISTINCT c.assignment_id) = 1
    ),
    affected_work_item_id = (
      SELECT MIN(a.work_item_id)
      FROM finding_deliveries d
      JOIN commands c ON c.project_id = d.project_id AND c.command_id = d.command_id
      JOIN assignments a ON a.project_id = c.project_id AND a.assignment_id = c.assignment_id
      WHERE d.project_id = findings.project_id AND d.finding_id = findings.finding_id
        AND d.seat_id = a.seat_id
      HAVING COUNT(DISTINCT c.assignment_id) = 1
    ),
    affected_generation = (
      SELECT MIN(c.generation)
      FROM finding_deliveries d
      JOIN commands c ON c.project_id = d.project_id AND c.command_id = d.command_id
      JOIN assignments a ON a.project_id = c.project_id AND a.assignment_id = c.assignment_id
      WHERE d.project_id = findings.project_id AND d.finding_id = findings.finding_id
        AND d.seat_id = a.seat_id
      HAVING COUNT(DISTINCT c.assignment_id) = 1
        AND COUNT(DISTINCT c.generation) = 1
    )
WHERE affected_assignment_id IS NULL
  AND state <> 'resolved'
  AND 1 = (
    SELECT COUNT(DISTINCT c.assignment_id || ':' || c.generation)
    FROM finding_deliveries d
    JOIN commands c ON c.project_id = d.project_id AND c.command_id = d.command_id
    JOIN assignments a ON a.project_id = c.project_id AND a.assignment_id = c.assignment_id
    WHERE d.project_id = findings.project_id AND d.finding_id = findings.finding_id
      AND d.seat_id = a.seat_id
  );
