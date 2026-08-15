-- 22.3: `state` says what a *connection* is doing, so a row without one has no
-- state at all.
--
-- 0012 made `state` NOT NULL, and `begin_link` therefore had to put something in
-- the column before anything was connected. It put 'error'. The consequence was
-- measured on a running panel: a user who had only pressed "Connect" read
-- `state: error, last_error: null`, and the operator's overview showed "error"
-- for every such account without a reason to go with it. An account that was
-- never connected is not a fault, and neither is one that is connecting right
-- now — that one has `link_state`, which is where the attempt belongs (the same
-- split as playit's `agent.state` beside its `claim`).
--
-- SQLite cannot loosen a CHECK in place, so the table is rebuilt. Nothing
-- references drive_accounts, which is what makes this a copy and not a surgery.

CREATE TABLE drive_accounts_new (
    user_id             TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    google_name         TEXT,
    google_email        TEXT,
    folder_id           TEXT,
    -- NULL = nothing is connected to this account: it never was, the attempt is
    -- still running, or the last one did not come good (`last_error` says which).
    -- 'connected' is a token that works, 'revoked' one Google answered
    -- invalid_grant to, 'error' a connection that stopped working for any other
    -- reason.
    state               TEXT CHECK (state IS NULL
                                    OR state IN ('connected', 'revoked', 'error')),
    storage_limit_bytes INTEGER CHECK (storage_limit_bytes IS NULL OR storage_limit_bytes >= 0),
    storage_usage_bytes INTEGER CHECK (storage_usage_bytes IS NULL OR storage_usage_bytes >= 0),
    link_user_code      TEXT,
    link_state          TEXT CHECK (link_state IN ('waiting', 'accepted', 'denied', 'expired')),
    link_started_at     TEXT,
    link_expires_at     TEXT,
    checked_at          TEXT,
    last_error          TEXT,
    updated_at          TEXT NOT NULL,
    CHECK ((link_user_code IS NULL) = (link_state IS NULL)),
    CHECK ((link_user_code IS NULL) = (link_started_at IS NULL))
);

-- The two rows this panel already has have to move with it. 'error' without a
-- reason and without a check ever having run is exactly what 0012's default left
-- behind: no other path writes 'error' — `record_error` always writes a sentence
-- and a `checked_at` with it — so this condition catches the stubs and nothing
-- that ever really failed.
INSERT INTO drive_accounts_new
SELECT user_id, google_name, google_email, folder_id,
       CASE WHEN state = 'error' AND last_error IS NULL AND checked_at IS NULL
            THEN NULL ELSE state END,
       storage_limit_bytes, storage_usage_bytes, link_user_code, link_state,
       link_started_at, link_expires_at, checked_at, last_error, updated_at
  FROM drive_accounts;

DROP TABLE drive_accounts;
ALTER TABLE drive_accounts_new RENAME TO drive_accounts;
