-- Which version of the battle rules (godcore::sim::SIM_VERSION) a base was
-- published under. Raids only run against bases from the current rules, so a
-- rules change never replays an old base differently from how it was fought.
ALTER TABLE snapshots ADD COLUMN sim_version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE attacks ADD COLUMN sim_version INTEGER NOT NULL DEFAULT 1;
