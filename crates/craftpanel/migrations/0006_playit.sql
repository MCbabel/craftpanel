-- playit.gg: a public address for every server, without anyone touching a
-- router. See docs/PLAYIT.md; what matters here is that the secret key is
-- deliberately absent. It lives in <data_dir>/playit/secret with mode 0600, so a
-- copy of this database — for a bug report, for a backup — carries no way in to
-- somebody else's service.

CREATE TABLE playit_account (
    id               INTEGER PRIMARY KEY CHECK (id = 1),
    -- playit's own UUID for the agent, not a ULID. It comes back from
    -- /v1/agents/rundata and is sent along when a tunnel is created.
    agent_id         TEXT,
    account_status   TEXT    CHECK (account_status IN ('guest', 'email_not_verified',
                                                       'verified')),
    is_self_managed  INTEGER NOT NULL DEFAULT 0 CHECK (is_self_managed IN (0, 1)),
    has_premium      INTEGER NOT NULL DEFAULT 0 CHECK (has_premium IN (0, 1)),
    -- The claim in progress. All three set together or all three null, so a
    -- half-written claim cannot outlive the request that started it.
    claim_code       TEXT,
    claim_state      TEXT    CHECK (claim_state IN ('waiting_for_visit', 'waiting_for_user',
                                                    'accepted', 'rejected')),
    claim_started_at TEXT,
    checked_at       TEXT,
    last_error       TEXT,
    updated_at       TEXT    NOT NULL,
    CHECK ((claim_code IS NULL) = (claim_state IS NULL)),
    CHECK ((claim_code IS NULL) = (claim_started_at IS NULL))
);

INSERT INTO playit_account (id, is_self_managed, has_premium, updated_at)
VALUES (1, 0, 0, '1970-01-01T00:00:00Z');

-- One tunnel per server, and that is the primary key: two tunnels onto the same
-- server would be two of four free ports spent on nothing.
CREATE TABLE playit_tunnels (
    server_id   TEXT PRIMARY KEY REFERENCES servers(id) ON DELETE CASCADE,
    -- playit's UUID. Null while the tunnel is being created.
    tunnel_id   TEXT UNIQUE,
    -- Always from allocations.is_primary, never from a request. PLAYIT.md 2.3:
    -- this number is a hole from the internet onto a port of this machine.
    local_port  INTEGER NOT NULL CHECK (local_port BETWEEN 1024 AND 65535),
    state       TEXT NOT NULL CHECK (state IN ('pending', 'online', 'offline',
                                               'missing', 'failed')),
    -- What the players type, exactly as playit hands it over. JSON, because
    -- connect_addresses is a list and none of it is thrown away.
    addresses   TEXT NOT NULL DEFAULT '[]',
    detail      TEXT,
    created_at  TEXT NOT NULL,
    checked_at  TEXT
);

-- Deleting a server must never be held up by a service of somebody else's, so
-- the row above goes away with the server. The tunnel on playit's side would
-- then keep one of four ports for good, unreachable from this panel — the
-- trigger writes the id down instead, and the reconcile loop hands it back.
CREATE TABLE playit_released (
    tunnel_id   TEXT PRIMARY KEY,
    released_at TEXT NOT NULL
);

CREATE TRIGGER playit_tunnel_released
AFTER DELETE ON playit_tunnels
WHEN OLD.tunnel_id IS NOT NULL
BEGIN
    INSERT OR IGNORE INTO playit_released (tunnel_id, released_at)
    VALUES (OLD.tunnel_id, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'));
END;
