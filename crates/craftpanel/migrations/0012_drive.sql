-- Section 22: backups into the user's own Google Drive. Three new tables, four
-- columns on `backups`.
--
-- The shape is playit's, one account per panel user (18): the operator sets up
-- one Google project, and every user connects his own Google account. The panel
-- provides none of its own — the consent happens in the browser of the person the
-- account belongs to.
--
-- Secrets are absent from this database for the same reason as in 0008: the
-- panel-wide client secret lives in <data_dir>/drive/client_secret, the refresh
-- token of a user in <data_dir>/drive/<user_id>/refresh_token, both 0600 in a
-- 0700 directory. The access token exists only in memory.


-- ------------------------------------------------------- Panel-Einstellungen
--
-- A table of its own and deliberately not panel_settings: that row is written by
-- 12.11 as a whole, and a second area writing into it means auth/settings.rs,
-- api/admin.rs and admin/Settings.vue all get a second hand on them. The client
-- id is not a secret; the secret is a file.

CREATE TABLE drive_settings (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    -- NULL = the operator has set nothing up. That is a normal state, not an
    -- error: every drive endpoint answers 409 drive_not_configured and the target
    -- `drive` appears in no menu (22.2).
    client_id     TEXT,
    target_policy TEXT    NOT NULL DEFAULT 'user_choice'
                          CHECK (target_policy IN ('user_choice', 'drive_only', 'local_only')),
    folder_name   TEXT    NOT NULL DEFAULT 'mcpanel-backups',
    updated_at    TEXT    NOT NULL
);

INSERT INTO drive_settings (id, updated_at) VALUES (1, '1970-01-01T00:00:00Z');


-- ------------------------------------------------------- Konto je Nutzer
--
-- No seed row: a user without a row has connected nothing, and that is the
-- ordinary case for every account this panel will ever have (0008 says the same
-- about playit_accounts).

CREATE TABLE drive_accounts (
    user_id             TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    google_name         TEXT,
    google_email        TEXT,
    -- Google's own file id of the folder, not a ULID.
    folder_id           TEXT,
    -- 'revoked' is what a refresh answered with invalid_grant: the user withdrew
    -- access, or the consent screen is still on "Testing" and the token expired
    -- after seven days (22.3). The key file stays where it is — the state belongs
    -- in this column, not in the presence of a file (playit/mod.rs:103-104).
    state               TEXT NOT NULL CHECK (state IN ('connected', 'revoked', 'error')),
    -- about.get storageQuota; NULL limit = unlimited (Workspace).
    storage_limit_bytes INTEGER CHECK (storage_limit_bytes IS NULL OR storage_limit_bytes >= 0),
    storage_usage_bytes INTEGER CHECK (storage_usage_bytes IS NULL OR storage_usage_bytes >= 0),
    -- The device flow in progress: all three set together or all three null, so a
    -- half-written attempt cannot outlive the request that started it.
    --
    -- Whoever confirms a user_code decides whose Drive the backups of a panel
    -- account land in. It is therefore a secret of its owner: it never appears in
    -- an answer that is not his, never in the admin overview (22.9), never in a
    -- log line. The device_code is not here at all — it is the voucher the token
    -- is fetched with and lives only in the polling loop, so a restart throws an
    -- unfinished attempt away and the user presses again.
    link_user_code      TEXT,
    link_state          TEXT CHECK (link_state IN ('waiting', 'accepted', 'denied', 'expired')),
    link_started_at     TEXT,
    -- Google's expires_in, not a deadline of ours.
    link_expires_at     TEXT,
    checked_at          TEXT,
    last_error          TEXT,
    updated_at          TEXT NOT NULL,
    CHECK ((link_user_code IS NULL) = (link_state IS NULL)),
    CHECK ((link_user_code IS NULL) = (link_started_at IS NULL))
);


-- ------------------------------------------------------------- Ziel je Server
--
-- Its own table like backup_schedules, not a column on `servers`: no row means
-- `local`, and `servers` is written by every other area of the panel.

CREATE TABLE backup_targets (
    server_id  TEXT PRIMARY KEY REFERENCES servers(id) ON DELETE CASCADE,
    target     TEXT NOT NULL CHECK (target IN ('local', 'drive')),
    updated_at TEXT NOT NULL
);


-- --------------------------------------------------------------- Sicherungen
--
-- DEFAULT 'local' is the reason no existing backup has to be touched: every row
-- from 0002 is local and stays readable, downloadable and restorable exactly as
-- it is.
--
-- Deliberately no CHECK "location = 'drive' implies drive_file_id NOT NULL": while
-- the run is uploading there is no id yet.

ALTER TABLE backups ADD COLUMN location TEXT NOT NULL DEFAULT 'local'
                            CHECK (location IN ('local', 'drive'));
ALTER TABLE backups ADD COLUMN drive_file_id TEXT;
ALTER TABLE backups ADD COLUMN drive_state TEXT
                            CHECK (drive_state IS NULL OR drive_state IN ('present', 'missing',
                                                                          'trashed', 'unreachable'));
ALTER TABLE backups ADD COLUMN drive_checked_at TEXT;

CREATE INDEX backups_drive_file ON backups(drive_file_id) WHERE drive_file_id IS NOT NULL;

-- The disk limit of 12.7 counts bytes that lie on *this* machine. A finished
-- drive backup keeps its size_bytes — 10.1 shows the size and the user wants to
-- see it — but auth/disk.rs must add `AND b.location = 'local'` to its sum, or a
-- backup that left for Google would hold a share of the account's disk quota for
-- good (22.8).
