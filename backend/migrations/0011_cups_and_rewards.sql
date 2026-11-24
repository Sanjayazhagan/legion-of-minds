-- More admin controls: gifts that give weapons or a trained troop, not just
-- gold, and one-off tournaments next to the weekly cup. The weekly cup's
-- prizes and closing time are settings (the settings table from 0006).

-- Gifts: gold may now be 0 when the gift gives something else.
ALTER TABLE gifts DROP CONSTRAINT IF EXISTS gifts_gold_check;
ALTER TABLE gifts ADD COLUMN weapons TEXT NOT NULL DEFAULT '';  -- comma-separated weapon names
ALTER TABLE gifts ADD COLUMN troop TEXT;                        -- troop JSON, as the game saves it
ALTER TABLE gifts ADD COLUMN troop_name TEXT;
ALTER TABLE gifts ADD CONSTRAINT gifts_gives_something
    CHECK (gold >= 0 AND (gold > 0 OR weapons <> '' OR troop IS NOT NULL));

-- One-off tournaments, scored like the weekly cup.
CREATE TABLE tournaments (
    id         UUID PRIMARY KEY,
    title      TEXT NOT NULL,
    message    TEXT NOT NULL DEFAULT '',
    starts_at  TIMESTAMPTZ NOT NULL,
    ends_at    TIMESTAMPTZ NOT NULL,
    category   SMALLINT,           -- only this brain-size category, or any
    class      TEXT,               -- only this main class, or any
    prizes     TEXT NOT NULL,      -- JSON list of prize bands (may hold troops)
    paid_at    TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (ends_at > starts_at)
);
CREATE INDEX tournaments_unpaid ON tournaments (ends_at) WHERE paid_at IS NULL;

CREATE TABLE tournament_entries (
    tournament_id UUID NOT NULL REFERENCES tournaments (id) ON DELETE CASCADE,
    user_id       UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    troop         TEXT NOT NULL,
    troop_name    TEXT NOT NULL,
    class_name    TEXT NOT NULL,
    score         REAL NOT NULL,
    category      SMALLINT NOT NULL,
    entered_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Set when prizes are handed out.
    rank          INTEGER,
    prize         BIGINT,
    PRIMARY KEY (tournament_id, user_id)
);
CREATE INDEX tournament_entries_rank ON tournament_entries (tournament_id, category, score DESC);
