CREATE TABLE users (
    id               UUID PRIMARY KEY,
    display_name     TEXT NOT NULL,
    trophies         INTEGER NOT NULL DEFAULT 0 CHECK (trophies >= 0),
    gold             BIGINT NOT NULL DEFAULT 0 CHECK (gold >= 0),
    shield_until     TIMESTAMPTZ,
    current_snapshot UUID,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX users_trophies ON users (trophies DESC);

-- One row per login method. A user can link several (Google and Discord).
CREATE TABLE identities (
    provider   TEXT NOT NULL,
    subject    TEXT NOT NULL,
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (provider, subject)
);

-- Only the SHA-256 of each bearer token is stored.
CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY,
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX sessions_user ON sessions (user_id);

-- A browser sign-in in progress. The game polls with a secret until the
-- provider callback fills in user_id.
CREATE TABLE oauth_logins (
    id            UUID PRIMARY KEY,
    provider      TEXT NOT NULL,
    state         TEXT NOT NULL UNIQUE,
    pkce_verifier TEXT NOT NULL,
    poll_hash     TEXT NOT NULL,
    user_id       UUID REFERENCES users (id) ON DELETE CASCADE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at    TIMESTAMPTZ NOT NULL
);

-- Published bases. Immutable: attacks point at the snapshot they fought, so
-- replays still work after the owner publishes a new one. `data` is the
-- exact JSON (TEXT, not JSONB, so floats come back bit-for-bit).
CREATE TABLE snapshots (
    id         UUID PRIMARY KEY,
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    town_hall  INTEGER NOT NULL,
    data       TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX snapshots_user ON snapshots (user_id);

ALTER TABLE users
    ADD CONSTRAINT users_current_snapshot
    FOREIGN KEY (current_snapshot) REFERENCES snapshots (id) ON DELETE SET NULL;

CREATE TABLE attacks (
    id                 UUID PRIMARY KEY,
    attacker_id        UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    defender_id        UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    snapshot_id        UUID NOT NULL REFERENCES snapshots (id) ON DELETE CASCADE,
    status             TEXT NOT NULL CHECK (status IN ('pending', 'done', 'expired')),
    army               TEXT,
    stars              INTEGER,
    destruction        REAL,
    town_hall_destroyed BOOLEAN,
    ticks              INTEGER,
    fingerprint        TEXT,
    client_fingerprint TEXT,
    client_matched     BOOLEAN,
    trophies           INTEGER,
    gold_looted        BIGINT,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at         TIMESTAMPTZ NOT NULL,
    finished_at        TIMESTAMPTZ
);
CREATE INDEX attacks_defender ON attacks (defender_id, finished_at DESC);
CREATE INDEX attacks_attacker ON attacks (attacker_id, finished_at DESC);
CREATE INDEX attacks_pending ON attacks (defender_id) WHERE status = 'pending';
