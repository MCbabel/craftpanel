-- playit.gg: one account per panel user. The panel provides none of its own any
-- more — whoever wants a public address for his servers connects his own account
-- at playit.gg, and his four ports are his.
--
-- The key is still deliberately absent from this database. It now lives in
-- <data_dir>/playit/<user_id>/secret with mode 0600 in a 0700 directory, so a
-- copy of this file — for a bug report, for a backup — still carries no way in
-- to somebody else's service.

CREATE TABLE playit_accounts (
    user_id          TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
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

-- No seed row: a user without a row has connected nothing, and that is the
-- ordinary case for every account this panel will ever have.

-- playit_tunnels gets a user, and it has to be NOT NULL — a tunnel whose owner
-- is unknown could not be handed back, because the key it was made with is the
-- key of exactly one account. SQLite cannot add a NOT NULL column with a
-- reference, so the table is rebuilt. The trigger goes first: while it exists it
-- reads a column layout that is about to change.
DROP TRIGGER playit_tunnel_released;

CREATE TABLE playit_tunnels_new (
    server_id   TEXT PRIMARY KEY REFERENCES servers(id) ON DELETE CASCADE,
    -- Whose playit account carries this tunnel and pays one of its ports. Always
    -- the owner of the server, even when a panel administrator pressed the button.
    user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    tunnel_id   TEXT UNIQUE,
    local_port  INTEGER NOT NULL CHECK (local_port BETWEEN 1024 AND 65535),
    state       TEXT NOT NULL CHECK (state IN ('pending', 'online', 'offline',
                                               'missing', 'failed')),
    addresses   TEXT NOT NULL DEFAULT '[]',
    detail      TEXT,
    created_at  TEXT NOT NULL,
    checked_at  TEXT
);

INSERT INTO playit_tunnels_new
    (server_id, user_id, tunnel_id, local_port, state, addresses, detail, created_at, checked_at)
SELECT server_id,
       (SELECT owner_id FROM servers WHERE servers.id = playit_tunnels.server_id),
       tunnel_id, local_port, state, addresses, detail, created_at, checked_at
  FROM playit_tunnels;

DROP TABLE playit_tunnels;
ALTER TABLE playit_tunnels_new RENAME TO playit_tunnels;
CREATE INDEX playit_tunnels_user ON playit_tunnels(user_id);

-- The debt of 0006 keeps its shape and gains the one thing that makes it
-- payable: whose key it has to be handed back with. No reference to users here,
-- on purpose — a debt that vanished with the account would leave a port occupied
-- on a stranger's account for good.
CREATE TABLE playit_released_new (
    user_id     TEXT NOT NULL,
    tunnel_id   TEXT PRIMARY KEY,
    released_at TEXT NOT NULL
);

INSERT INTO playit_released_new (user_id, tunnel_id, released_at)
SELECT (SELECT id FROM users WHERE role = 'admin' ORDER BY created_at, id LIMIT 1),
       tunnel_id, released_at
  FROM playit_released
 WHERE (SELECT id FROM users WHERE role = 'admin' ORDER BY created_at, id LIMIT 1) IS NOT NULL;

DROP TABLE playit_released;
ALTER TABLE playit_released_new RENAME TO playit_released;

CREATE TRIGGER playit_tunnel_released
AFTER DELETE ON playit_tunnels
WHEN OLD.tunnel_id IS NOT NULL
BEGIN
    INSERT OR IGNORE INTO playit_released (user_id, tunnel_id, released_at)
    VALUES (OLD.user_id, OLD.tunnel_id, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'));
END;

-- playit_account of 0006 stays exactly where it is, untouched. The panel-wide
-- key is adopted by the oldest administrator at startup (playit/legacy.rs), and
-- an adoption that goes wrong has to remain readable afterwards. A later
-- migration may drop the table once nobody needs to look.
