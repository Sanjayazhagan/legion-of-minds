-- Anonymous play data from players who opted in (see bag_protocol::playdata).
-- No account, name or address: only the random id the game made when the
-- player said yes. Rows older than PLAY_KEEP_DAYS are deleted by cleanup.
CREATE TABLE play_events (
    id           BIGSERIAL PRIMARY KEY,
    install_id   UUID NOT NULL,
    session_id   UUID NOT NULL,
    name         TEXT NOT NULL,
    -- Seconds since that game session started.
    secs         INTEGER NOT NULL,
    props        JSONB NOT NULL DEFAULT '{}',
    game_version TEXT NOT NULL,
    sim_version  INTEGER NOT NULL,
    os           TEXT NOT NULL,
    received_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX play_events_name_at ON play_events (name, received_at);
CREATE INDEX play_events_install ON play_events (install_id, session_id);
CREATE INDEX play_events_at ON play_events (received_at);

-- Read a number or a true/false from an event's details, or NULL if the
-- detail is missing or of another type, so one odd event can't break the
-- admin page's sums.
CREATE FUNCTION play_num(j JSONB, k TEXT) RETURNS DOUBLE PRECISION
    LANGUAGE sql IMMUTABLE AS
    $$ SELECT CASE WHEN jsonb_typeof(j -> k) = 'number' THEN (j ->> k)::double precision END $$;
CREATE FUNCTION play_bool(j JSONB, k TEXT) RETURNS BOOLEAN
    LANGUAGE sql IMMUTABLE AS
    $$ SELECT CASE WHEN jsonb_typeof(j -> k) = 'boolean' THEN (j ->> k)::boolean END $$;
