-- Section 20: people can sign themselves up. Until now the only way in was
-- `mcpanel admin create` (auth/cli.rs:22, "There is no registration; this is the
-- way in."), and that sentence is no longer the whole truth — it stays the way to
-- the *first* administrator.
--
-- The tragende Entscheidung of this file: an open application is NOT a row in
-- `users`. It is a row in `registrations`, and the `users` row is created only
-- when the account becomes usable. Three reasons out of the existing code:
--
--   * users::reconcile (auth/users.rs:463-484) walks every row with
--     system_state = 'provisioning' at each panel start and creates a system user
--     for it. A half-finished application in `users` would get the very system
--     account it must not get, on the next restart.
--   * users::search (auth/users.rs:134) serves GET /users/search (3.5), the
--     invite path onto other people's servers. An account without a system user
--     would be invitable.
--   * page(), promised() and the disk sum (auth/users.rs:104, api/admin.rs:210,
--     auth/disk.rs) would count half accounts. HostCapacity.allocated would claim
--     memory nobody was ever given.
--
-- With its own table not one of those queries changes.


-- ---------------------------------------------------------- Die Bewerbung

CREATE TABLE registrations (
    id               TEXT    PRIMARY KEY CHECK (length(id) = 26),
    -- Lower case, rules of 12.3. UNIQUE holds the name while the applicant reads
    -- his mail; users::claim_name asks this table too (20.3).
    username         TEXT    NOT NULL UNIQUE,
    -- Normal form: trimmed, lower case, at most 254 characters (20.4).
    email            TEXT    NOT NULL UNIQUE,
    -- argon2id, moved into the users row at admission. It is hashed at signup
    -- because a rejected application must not leave a readable password behind.
    password_hash    TEXT    NOT NULL,
    state            TEXT    NOT NULL CHECK (state IN ('email_unverified', 'awaiting_approval')),
    -- SHA-256 of the confirmation token, never the token, exactly as
    -- sessions.token_hash (0002_schema.sql:52-53).
    token_hash       TEXT,
    token_expires_at TEXT,
    -- For the five-minute brake on "send it again".
    token_sent_at    TEXT,
    tokens_sent      INTEGER NOT NULL DEFAULT 0 CHECK (tokens_sent >= 0),
    -- The only trace for triage when five applications arrive from one address.
    -- Cleared at admission; it has no business in a working account.
    signup_ip        TEXT,
    -- When the address was confirmed. There is no second "consumed_at" column:
    -- it would hold the same fact. A second click on the same link answers again
    -- as long as this row lives — with approval switched on it does, and with
    -- approval off the row is already gone and the answer is 404 with a sentence
    -- that says so (20.3). No mail scanner burns a token either way: the link is
    -- a GET on a page of our interface, and the redemption is a POST that only a
    -- real browser makes. The password reset token must NOT copy this leniency
    -- (21.5).
    verified_at      TEXT,
    created_at       TEXT    NOT NULL,
    updated_at       TEXT    NOT NULL,
    CHECK ((token_hash IS NULL) = (token_expires_at IS NULL)),
    CHECK ((state = 'awaiting_approval') = (verified_at IS NOT NULL))
);

-- NULL stays free, as with playit_tunnels.tunnel_id in 0008.
CREATE UNIQUE INDEX registrations_token_hash ON registrations(token_hash);
CREATE INDEX registrations_created_at ON registrations(created_at);

-- There is deliberately no role column and no limit column here. That is the
-- structural guarantee that somebody who signs himself up can never be an
-- administrator: there is nothing in this table that could carry a role. Limits
-- are the panel defaults at the moment of admission, not at the moment of the
-- form.


-- ------------------------------------------------------ Ablehnung und Sperre

CREATE TABLE registration_blocks (
    email      TEXT PRIMARY KEY,
    -- NULL = for good; that is the operator's own block. A rejection sets it 30
    -- days out so the same applicant is not back in the list tomorrow.
    until      TEXT,
    -- Stays inside the panel. The rejection mail carries no reason (20.7).
    reason     TEXT,
    created_at TEXT NOT NULL
);


-- ------------------------------------------------------------------ Konten
--
-- Two columns on `users`, both harmless for every row that exists: an account
-- made by hand has no address, and it was made by an administrator.

ALTER TABLE users ADD COLUMN email TEXT;
ALTER TABLE users ADD COLUMN origin TEXT NOT NULL DEFAULT 'admin'
                             CHECK (origin IN ('admin', 'registration'));

-- Several NULLs are allowed in SQLite, so accounts without an address keep
-- working while addresses stay unique.
CREATE UNIQUE INDEX users_email ON users(email);

-- No email_verified_at column, on purpose. Every address that reaches `users` is
-- usable by construction: either its owner clicked the link (this section) or an
-- administrator typed it (12.3, 12.5). The unconfirmed state lives in
-- `registrations` and nowhere else, so a column here would have two meanings —
-- and 0002 already wrote down what that costs.


-- --------------------------------------------------- Panel-Einstellungen
--
-- 12.10 gains two switches. Both defaults are the closed door: an update must not
-- quietly open a running panel, and if the operator opens it, the safe setting is
-- that he sees every account before it works.

ALTER TABLE panel_settings ADD COLUMN registration_enabled INTEGER NOT NULL DEFAULT 0
                                      CHECK (registration_enabled IN (0, 1));
ALTER TABLE panel_settings ADD COLUMN registration_requires_approval INTEGER NOT NULL DEFAULT 1
                                      CHECK (registration_requires_approval IN (0, 1));
