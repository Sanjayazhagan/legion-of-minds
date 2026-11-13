-- Practice raids, revenge, gold rewards and the weekly cup.

-- 'raid' is a matched raid; 'practice' is offered when nobody can be raided
-- (it costs the defender nothing); 'revenge' answers a raid from the log.
ALTER TABLE attacks ADD COLUMN kind TEXT NOT NULL DEFAULT 'raid'
    CHECK (kind IN ('raid', 'practice', 'revenge'));
-- Gold the server paid the attacker on top of loot (daily first win, practice).
ALTER TABLE attacks ADD COLUMN bonus_gold BIGINT NOT NULL DEFAULT 0;
-- Gold the server paid the defender for holding the raid to no stars.
ALTER TABLE attacks ADD COLUMN defence_reward BIGINT NOT NULL DEFAULT 0;
-- Set once the defender has taken their revenge for this raid.
ALTER TABLE attacks ADD COLUMN revenged BOOLEAN NOT NULL DEFAULT FALSE;
-- For a revenge: the raid it answers.
ALTER TABLE attacks ADD COLUMN revenge_of UUID;
CREATE INDEX attacks_best ON attacks (finished_at DESC) WHERE status = 'done' AND kind <> 'practice';

-- The day (UTC) the player last got the first-win bonus.
ALTER TABLE users ADD COLUMN last_win_bonus DATE;
-- The Training Camp: owner of the computer-made bases used for practice
-- raids when no player has a base. Nobody can sign in as it.
ALTER TABLE users ADD COLUMN is_bot BOOLEAN NOT NULL DEFAULT FALSE;
INSERT INTO users (id, display_name, is_bot)
VALUES ('00000000-0000-0000-0000-000000000001', 'Training Camp', TRUE);

-- The weekly cup: one troop per player per week, scored by the same seeded
-- proving-grounds raids the game's own Tournament runs. `troop` is the
-- checked troop as JSON, so anyone can replay an entry.
CREATE TABLE cup_entries (
    week       INTEGER NOT NULL,
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    troop      TEXT NOT NULL,
    troop_name TEXT NOT NULL,
    class_name TEXT NOT NULL,
    score      REAL NOT NULL,
    entered_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Gold paid when the week closed; NULL while it is open.
    prize      BIGINT,
    PRIMARY KEY (week, user_id)
);
CREATE INDEX cup_week ON cup_entries (week, score DESC, entered_at);
