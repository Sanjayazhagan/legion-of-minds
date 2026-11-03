-- What each player has paid for with online gold. The server checks every
-- published or raiding troop against these, so upgrades bought offline or
-- edited into a save file don't count online.
ALTER TABLE users ADD COLUMN town_hall_level INTEGER NOT NULL DEFAULT 1
    CHECK (town_hall_level BETWEEN 1 AND 5);

CREATE TABLE user_weapons (
    user_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    weapon  TEXT NOT NULL,
    PRIMARY KEY (user_id, weapon)
);

-- Troops start at level 1, so only levelled troops have a row. `troop_id`
-- is the id the game gave the troop.
CREATE TABLE troop_levels (
    user_id  UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    troop_id BIGINT NOT NULL,
    level    INTEGER NOT NULL CHECK (level BETWEEN 2 AND 5),
    PRIMARY KEY (user_id, troop_id)
);
