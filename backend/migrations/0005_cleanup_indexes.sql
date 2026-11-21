-- Cleanup deletes snapshots nothing points at. Each deleted snapshot makes
-- Postgres look for raids that fought it (ON DELETE CASCADE) and players
-- whose current base it is (ON DELETE SET NULL); without these indexes that
-- is a full table scan per snapshot. With 50k players and 1M raids, one
-- cleanup took over 5 minutes before and under 3 seconds after.
CREATE INDEX attacks_snapshot ON attacks (snapshot_id);
CREATE INDEX users_current_snapshot ON users (current_snapshot);
