-- Tiredness that carries between raids (rules v10, stage C).

-- How tired each of a player's troops went home from its last raid. The
-- server computes this from its own re-run, never from the client.
CREATE TABLE troop_rest (
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    troop_id    BIGINT NOT NULL,
    carried     REAL NOT NULL,
    finished_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (user_id, troop_id)
);
-- How tired each troop was when the raid was opened (JSON list of
-- {troop_id, fatigue}); fixed then so the client can run the same raid.
ALTER TABLE attacks ADD COLUMN rest TEXT;
