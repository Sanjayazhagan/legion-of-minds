-- Seasons: trophies are partly reset when one ends and everyone is paid
-- online gold by the league they finished in.
CREATE TABLE seasons (
    number     INTEGER PRIMARY KEY,
    started_at TIMESTAMPTZ NOT NULL,
    ends_at    TIMESTAMPTZ NOT NULL,
    closed_at  TIMESTAMPTZ
);

CREATE TABLE season_results (
    season   INTEGER NOT NULL REFERENCES seasons (number) ON DELETE CASCADE,
    user_id  UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    trophies INTEGER NOT NULL,
    league   TEXT NOT NULL,
    rank     BIGINT NOT NULL,
    reward   BIGINT NOT NULL,
    PRIMARY KEY (season, user_id)
);
CREATE INDEX season_results_user ON season_results (user_id, season DESC);

-- Clans. `name_key` is the lower-case name, so names are unique ignoring case.
CREATE TABLE clans (
    id            UUID PRIMARY KEY,
    name          TEXT NOT NULL,
    name_key      TEXT NOT NULL UNIQUE,
    description   TEXT NOT NULL DEFAULT '',
    open          BOOLEAN NOT NULL DEFAULT true,
    war_searching BOOLEAN NOT NULL DEFAULT false,
    war_wins      INTEGER NOT NULL DEFAULT 0,
    war_losses    INTEGER NOT NULL DEFAULT 0,
    war_draws     INTEGER NOT NULL DEFAULT 0,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One clan per player.
CREATE TABLE clan_members (
    user_id   UUID PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    clan_id   UUID NOT NULL REFERENCES clans (id) ON DELETE CASCADE,
    role      TEXT NOT NULL CHECK (role IN ('leader', 'elder', 'member')),
    joined_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX clan_members_clan ON clan_members (clan_id);

CREATE TABLE clan_messages (
    id         BIGSERIAL PRIMARY KEY,
    clan_id    UUID NOT NULL REFERENCES clans (id) ON DELETE CASCADE,
    user_id    UUID REFERENCES users (id) ON DELETE SET NULL,
    author     TEXT NOT NULL,
    body       TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX clan_messages_clan ON clan_messages (clan_id, id DESC);

-- Troops a member lends to their clan. `data` is the checked troop JSON
-- (TEXT so its floats come back bit-for-bit). Leaving the clan drops them.
CREATE TABLE troop_loans (
    id         UUID PRIMARY KEY,
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    clan_id    UUID NOT NULL REFERENCES clans (id) ON DELETE CASCADE,
    troop_id   BIGINT NOT NULL,
    data       TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, troop_id)
);
CREATE INDEX troop_loans_clan ON troop_loans (clan_id);

-- Clan wars. Each member's base is frozen when the war is made, so both
-- sides attack the same copies the whole war.
CREATE TABLE wars (
    id          UUID PRIMARY KEY,
    clan_a      UUID NOT NULL REFERENCES clans (id) ON DELETE CASCADE,
    clan_b      UUID NOT NULL REFERENCES clans (id) ON DELETE CASCADE,
    size        INTEGER NOT NULL,
    sim_version INTEGER NOT NULL,
    battle_at   TIMESTAMPTZ NOT NULL,
    ends_at     TIMESTAMPTZ NOT NULL,
    -- Set once the war is scored and rewards are paid.
    winner      UUID,
    settled_at  TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX wars_clan_a ON wars (clan_a, created_at DESC);
CREATE INDEX wars_clan_b ON wars (clan_b, created_at DESC);

CREATE TABLE war_bases (
    war_id      UUID NOT NULL REFERENCES wars (id) ON DELETE CASCADE,
    user_id     UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    clan_id     UUID NOT NULL,
    position    INTEGER NOT NULL,
    snapshot_id UUID NOT NULL REFERENCES snapshots (id) ON DELETE CASCADE,
    PRIMARY KEY (war_id, user_id)
);

CREATE TABLE war_attacks (
    id             UUID PRIMARY KEY,
    war_id         UUID NOT NULL REFERENCES wars (id) ON DELETE CASCADE,
    attacker_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    defender_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    status         TEXT NOT NULL CHECK (status IN ('pending', 'done', 'expired')),
    army           TEXT,
    stars          INTEGER,
    destruction    REAL,
    new_stars      INTEGER,
    fingerprint    TEXT,
    client_matched BOOLEAN,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at     TIMESTAMPTZ NOT NULL,
    finished_at    TIMESTAMPTZ
);
CREATE INDEX war_attacks_war ON war_attacks (war_id, defender_id);
