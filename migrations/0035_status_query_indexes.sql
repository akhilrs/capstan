CREATE INDEX actors_by_seat_active ON actors(project_id, seat_id, active);
CREATE INDEX assignments_by_seat_authority ON assignments(project_id, seat_id, authority_state, created_at);
