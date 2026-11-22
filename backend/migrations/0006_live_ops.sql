-- Live operations: what the game's operator changes from the admin page
-- without releasing a new game build. (0005 belongs to the server fixes
-- from the player simulation.)

-- Rewards and timings that override the built-in defaults. A missing row
-- means the default.
CREATE TABLE settings (
    key        TEXT PRIMARY KEY,
    value      BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Time-limited events such as a double gold weekend. They only change
-- payouts, never battle rules.
CREATE TABLE events (
    id         UUID PRIMARY KEY,
    title      TEXT NOT NULL,
    message    TEXT NOT NULL DEFAULT '',
    starts_at  TIMESTAMPTZ NOT NULL,
    ends_at    TIMESTAMPTZ NOT NULL,
    gold_pct   INTEGER NOT NULL DEFAULT 100,
    win_gold   BIGINT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (ends_at > starts_at)
);
CREATE INDEX events_ends ON events (ends_at);

-- Gold handed out by the operator. user_id NULL means every player.
CREATE TABLE gifts (
    id         UUID PRIMARY KEY,
    user_id    UUID REFERENCES users (id) ON DELETE CASCADE,
    gold       BIGINT NOT NULL CHECK (gold > 0),
    message    TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX gifts_expires ON gifts (expires_at);

CREATE TABLE gift_claims (
    gift_id    UUID NOT NULL REFERENCES gifts (id) ON DELETE CASCADE,
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    claimed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (gift_id, user_id)
);

-- What was changed from the admin page, newest last.
CREATE TABLE admin_log (
    id     BIGSERIAL PRIMARY KEY,
    at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    action TEXT NOT NULL,
    detail TEXT NOT NULL DEFAULT ''
);

ALTER TABLE seasons ADD COLUMN name TEXT NOT NULL DEFAULT '';
