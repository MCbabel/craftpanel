-- Section 21: a forgotten password can be recovered over the address on the
-- account. One short-lived table.
--
-- Why the token lives in this database and not in a file with mode 0600 like
-- playit's key (0008): playit's key is used *outbound* — the panel reads it and
-- calls a foreign service with it. A reset token has to be found *inbound*, by
-- its own value, and looking that up over a tree of files would be a table scan
-- written by hand. It goes into a table, and there only as a digest — which
-- keeps the property that made the file worth it: a copy of panel.db carries no
-- way in.

CREATE TABLE password_resets (
    id           TEXT PRIMARY KEY CHECK (length(id) = 26),
    user_id      TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- SHA-256 hex of 256 random bits in base64url, the same shape as
    -- sessions.token_hash (0002_schema.sql:52-53). No argon2: 256 real random
    -- bits are not a password, they are not guessable.
    token_hash   TEXT NOT NULL CHECK (length(token_hash) = 64),
    created_at   TEXT NOT NULL,
    expires_at   TEXT NOT NULL,
    -- Set = spent. The row stays until it is swept, so a second click gets the
    -- same answer as an unknown token instead of a fresh link.
    used_at      TEXT,
    -- The only trace when somebody uses the form to bother a stranger.
    requested_ip TEXT,
    user_agent   TEXT
);

CREATE UNIQUE INDEX password_resets_token_hash ON password_resets(token_hash);
-- The cool-down of 21.2 reads the newest rows of one account: at most one mail a
-- minute and five an hour, counted from created_at, so a restart does not hand
-- out a fresh allowance.
CREATE INDEX password_resets_user ON password_resets(user_id, created_at DESC);
CREATE INDEX password_resets_expires_at ON password_resets(expires_at);
