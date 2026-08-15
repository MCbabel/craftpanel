-- Everything docs/api/VERTRAG.md needs to keep between two requests. Section
-- numbers in the comments point there.
--
-- Two rules run through the whole file. Enum columns carry a CHECK with the
-- contract's exact spelling, so the database cannot hold a state the interface
-- would have to guess at; and every ULID column is 26 characters wide, which is
-- the second seam behind the Rust type that parses them.


-- ------------------------------------------------------------------ Konten
--
-- 0001 was written before the contract was settled. Three columns of it were
-- never read by any code path and are dropped rather than reinterpreted:
-- memory_limit and cpu_limit had no unit written down, and a limit in the wrong
-- unit is worse than no limit. system_state keeps its rows, translated into the
-- vocabulary of SystemUserState (12.3).

ALTER TABLE users ADD COLUMN last_login_at TEXT;
ALTER TABLE users ADD COLUMN must_change_password INTEGER NOT NULL DEFAULT 0
                             CHECK (must_change_password IN (0, 1));
ALTER TABLE users ADD COLUMN system_error_message TEXT;
-- 12.6: a user being deleted, moved or re-limited answers 409 user_busy in the
-- meantime, because chown -R under a running Java process is the kind of mistake
-- that only shows up weeks later.
ALTER TABLE users ADD COLUMN busy INTEGER NOT NULL DEFAULT 0 CHECK (busy IN (0, 1));

ALTER TABLE users ADD COLUMN account_state TEXT NOT NULL DEFAULT 'provisioning'
                             CHECK (account_state IN ('provisioning', 'ready', 'error'));
UPDATE users SET account_state = CASE system_state
    WHEN 'ready'  THEN 'ready'
    WHEN 'failed' THEN 'error'
    ELSE 'provisioning'
END;
ALTER TABLE users DROP COLUMN system_state;
ALTER TABLE users RENAME COLUMN account_state TO system_state;

ALTER TABLE users DROP COLUMN memory_limit;
ALTER TABLE users DROP COLUMN cpu_limit;
ALTER TABLE users DROP COLUMN pids_limit;

-- UserLimits (12.7): all four are mandatory, because 12.3 copies the panel
-- default into the row at creation instead of following it later.
ALTER TABLE users ADD COLUMN memory_mib INTEGER NOT NULL DEFAULT 4096
                             CHECK (memory_mib >= 512);
ALTER TABLE users ADD COLUMN cpu_mode TEXT NOT NULL DEFAULT 'cap'
                             CHECK (cpu_mode IN ('cap', 'share'));
ALTER TABLE users ADD COLUMN cpu_cores REAL NOT NULL DEFAULT 2.0 CHECK (cpu_cores > 0);
ALTER TABLE users ADD COLUMN pids_max INTEGER NOT NULL DEFAULT 512 CHECK (pids_max >= 64);

-- 1.2 stores the SHA-256 of the cookie, never the cookie. Me.session.id is a
-- ULID, so the two cannot share one column.
ALTER TABLE sessions ADD COLUMN token_hash TEXT NOT NULL DEFAULT '';
CREATE UNIQUE INDEX sessions_token_hash ON sessions(token_hash);


-- ------------------------------------------------------------------ Server

CREATE TABLE servers (
    id                   TEXT    PRIMARY KEY CHECK (length(id) = 26),
    name                 TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 64),
    -- 12.6 makes deleting a user with servers a decision, not a side effect.
    owner_id             TEXT    NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    status               TEXT    NOT NULL CHECK (status IN (
                                     'installing', 'available', 'broken', 'deleting')),
    loader               TEXT    CHECK (loader IS NULL OR loader IN (
                                     'vanilla', 'paper', 'folia', 'purpur', 'leaf',
                                     'fabric', 'velocity', 'neoforge', 'quilt', 'forge')),
    loader_version       TEXT,
    game_version         TEXT,
    memory_mib           INTEGER NOT NULL CHECK (memory_mib >= 512),
    update_channel       TEXT    NOT NULL DEFAULT 'release'
                                 CHECK (update_channel IN ('release', 'beta', 'alpha')),
    flows_intro          INTEGER NOT NULL DEFAULT 1 CHECK (flows_intro IN (0, 1)),
    -- 9.4: the startup command is a template, not a command line. What is stored
    -- is the choice of runtime and the flags the user added; argv is assembled
    -- from the loader definition at start.
    java_major           INTEGER CHECK (java_major IS NULL OR java_major > 0),
    jre_vendor           TEXT    CHECK (jre_vendor IS NULL
                                        OR jre_vendor IN ('temurin', 'corretto', 'graal')),
    extra_flags          TEXT    NOT NULL DEFAULT '[]' CHECK (json_valid(extra_flags)),
    -- 9.3: what the last PATCH threw out. Lives until the next one, because
    -- patchStartup discards its own response body.
    stripped_flags       TEXT    NOT NULL DEFAULT '[]' CHECK (json_valid(stripped_flags)),
    restart_required     INTEGER NOT NULL DEFAULT 0 CHECK (restart_required IN (0, 1)),
    -- docs/PLAN.md:236 — the helper issues this to both sides so a supervisor can
    -- prove who it is. It has to outlive a panel restart, or every running server
    -- loses its console on the next update. The hub compares it as it stands.
    supervisor_token     TEXT,
    -- 13.5: console line numbers count per server and are never reset.
    console_seq          INTEGER NOT NULL DEFAULT 0,
    -- 5.2: the counter that lets a provider throw away a snapshot it has already
    -- overtaken. Kept here so a reconnecting page is not handed a lower number.
    operations_revision  INTEGER NOT NULL DEFAULT 0,
    updates_checked_at   TEXT,
    created_at           TEXT    NOT NULL,
    updated_at           TEXT    NOT NULL
);

CREATE INDEX servers_owner_id ON servers(owner_id);

-- 9.6/9.7. One row per port, the primary one included, so the UNIQUE constraint
-- that turns into 409 port_in_use covers the whole panel and not one server.
CREATE TABLE allocations (
    port        INTEGER PRIMARY KEY NOT NULL CHECK (port BETWEEN 1024 AND 65535),
    server_id   TEXT    NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    name        TEXT    NOT NULL CHECK (length(name) BETWEEN 1 AND 32),
    is_primary  INTEGER NOT NULL DEFAULT 0 CHECK (is_primary IN (0, 1)),
    created_at  TEXT    NOT NULL
);

CREATE INDEX allocations_server_id ON allocations(server_id);
CREATE UNIQUE INDEX allocations_one_primary_per_server
    ON allocations(server_id) WHERE is_primary = 1;

-- 11.1: the owner is no membership row, he is read off servers.owner_id. Hence
-- no 'owner' here — 11.2 answers 400 role_not_assignable for it.
CREATE TABLE server_members (
    id                TEXT PRIMARY KEY CHECK (length(id) = 26),
    server_id         TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    user_id           TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role              TEXT NOT NULL CHECK (role IN ('editor', 'viewer')),
    invited_by        TEXT REFERENCES users(id) ON DELETE SET NULL,
    invited_at        TEXT NOT NULL,
    -- Null while the invitation is open: an invitation is a membership row
    -- without a joining date (11.6).
    joined_at         TEXT,
    last_invite_sent  TEXT,
    UNIQUE (server_id, user_id)
);

CREATE INDEX server_members_user_id ON server_members(user_id);


-- --------------------------------------------------------------- Sicherungen
--
-- Backups live outside the server directory (10), so nothing here holds a path:
-- it is <data>/backups/<server_id>/<id>.tar.zst and nothing else.

CREATE TABLE backups (
    id          TEXT    PRIMARY KEY CHECK (length(id) = 26),
    server_id   TEXT    NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    name        TEXT    NOT NULL CHECK (length(trim(name)) BETWEEN 1 AND 128),
    -- 10.12: only automatic backups are ever cleaned up by keep_last.
    automated   INTEGER NOT NULL DEFAULT 0 CHECK (automated IN (0, 1)),
    -- Zero until the archive is written; status is read off the runs (10.1).
    size_bytes  INTEGER NOT NULL DEFAULT 0 CHECK (size_bytes >= 0),
    created_at  TEXT    NOT NULL
);

CREATE INDEX backups_server_created_at ON backups(server_id, created_at DESC);

-- 10.9/10.10. One schedule per server, off by default.
CREATE TABLE backup_schedules (
    server_id       TEXT    PRIMARY KEY REFERENCES servers(id) ON DELETE CASCADE,
    enabled         INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    interval_hours  INTEGER NOT NULL DEFAULT 24 CHECK (interval_hours BETWEEN 1 AND 168),
    hour_utc        INTEGER NOT NULL DEFAULT 4 CHECK (hour_utc BETWEEN 0 AND 23),
    keep_last       INTEGER NOT NULL DEFAULT 5 CHECK (keep_last BETWEEN 1 AND 50),
    next_run_at     TEXT,
    last_run_at     TEXT,
    last_status     TEXT    CHECK (last_status IS NULL OR last_status IN (
                                'completed', 'failed', 'timed_out',
                                'skipped_unchanged', 'skipped_limit')),
    last_error      TEXT
);


-- ----------------------------------------------------------------- Vorgänge
--
-- Section 5: one table, one type, one WebSocket message for every long run in
-- the panel. A backup run is a row here too — BackupOperation is this row seen
-- through the backup adapter (10.1), which is why backups carry no state column.

CREATE TABLE operations (
    id                   TEXT    PRIMARY KEY CHECK (length(id) = 26),
    server_id            TEXT    NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    kind                 TEXT    NOT NULL CHECK (kind IN (
                                     'server_create', 'server_delete', 'install_loader',
                                     'repair_content', 'reset_server', 'install_modpack',
                                     'install_content', 'update_content', 'change_game_version',
                                     'install_java', 'backup_create', 'backup_restore',
                                     'unarchive')),
    state                TEXT    NOT NULL CHECK (state IN (
                                     'queued', 'ongoing', 'done', 'failed', 'cancelled')),
    phase                TEXT    CHECK (phase IS NULL OR phase IN (
                                     'analyzing', 'installing_loader', 'verifying',
                                     'running_installer', 'installing_pack', 'addons',
                                     'writing_config')),
    progress             REAL    NOT NULL DEFAULT 0 CHECK (progress BETWEEN 0 AND 1),
    message              TEXT,
    src                  TEXT,
    bytes_processed      INTEGER,
    files_processed      INTEGER,
    current_file         TEXT,
    error_code           TEXT,
    error_message        TEXT,
    error_step           TEXT    CHECK (error_step IS NULL OR error_step IN (
                                     'modloader', 'modpack', 'download', 'filesystem',
                                     'internal')),
    cancellable          INTEGER NOT NULL DEFAULT 0 CHECK (cancellable IN (0, 1)),
    -- The backup a backup_create or backup_restore works on.
    target_id            TEXT    REFERENCES backups(id) ON DELETE SET NULL,
    -- 10.1: set on the create run of a safety copy, and it points at the restore
    -- that asked for it. has_parent is this column being filled; 10.7 reuses the
    -- copy it finds here instead of stacking a second one into the quota.
    parent_operation_id  TEXT    REFERENCES operations(id) ON DELETE SET NULL,
    started_by           TEXT    REFERENCES users(id) ON DELETE SET NULL,
    -- What 5.6 needs to run the same thing again.
    input                TEXT    CHECK (input IS NULL OR json_valid(input)),
    -- 5.7: an upload that has not arrived yet. Three values, because
    -- payload_not_expected and payload_already_delivered are different answers.
    payload              TEXT    NOT NULL DEFAULT 'none'
                                 CHECK (payload IN ('none', 'expected', 'delivered')),
    -- 5.12: the point where an unarchive stops writing into its work directory
    -- and starts moving entries into place. Before it, a crash leaves nothing.
    applied_at           TEXT,
    created_at           TEXT    NOT NULL,
    started_at           TEXT,
    finished_at          TEXT,
    dismissed_at         TEXT,
    CHECK ((error_code IS NULL) = (error_step IS NULL)
           AND (error_code IS NULL) = (error_message IS NULL))
);

-- 5.2 pages descending by id, so the index has to as well.
CREATE INDEX operations_server_id ON operations(server_id, id DESC);
CREATE INDEX operations_open ON operations(server_id) WHERE state IN ('queued', 'ongoing');
CREATE INDEX operations_target_id ON operations(target_id);

-- 10.2 wants one open backup run per server as a second seam behind the
-- transaction. Two indexes and not one: a restore opens the create of its safety
-- copy alongside itself (10.6), and a single index would refuse exactly that.
CREATE UNIQUE INDEX operations_one_open_backup_create
    ON operations(server_id) WHERE kind = 'backup_create' AND state IN ('queued', 'ongoing');
CREATE UNIQUE INDEX operations_one_open_backup_restore
    ON operations(server_id) WHERE kind = 'backup_restore' AND state IN ('queued', 'ongoing');


-- ------------------------------------------------------------------ Inhalte

CREATE TABLE content_items (
    id                  TEXT    PRIMARY KEY CHECK (length(id) = 26),
    server_id           TEXT    NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    file_name           TEXT    NOT NULL,
    file_path           TEXT    NOT NULL,
    -- ApiContentItem calls this one `size`; the column keeps the _bytes of 1.5.
    size_bytes          INTEGER NOT NULL DEFAULT 0 CHECK (size_bytes >= 0),
    enabled             INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    -- Loader jar and server core: no deleting, no disabling.
    locked              INTEGER NOT NULL DEFAULT 0 CHECK (locked IN (0, 1)),
    project_type        TEXT    NOT NULL CHECK (project_type IN (
                                    'mod', 'plugin', 'datapack', 'resourcepack', 'shader')),
    source_kind         TEXT    NOT NULL CHECK (source_kind IN (
                                    'local', 'modrinth_modpack', 'server_project')),
    environment         TEXT,
    pack_client_depends INTEGER NOT NULL DEFAULT 0 CHECK (pack_client_depends IN (0, 1)),
    external            INTEGER NOT NULL DEFAULT 0 CHECK (external IN (0, 1)),
    external_url        TEXT,
    -- 8.17/9.15: repair compares this against the file on disk, so it is the one
    -- piece of the .mrpack index we have to keep after unpacking.
    sha512              TEXT,
    project_id          TEXT,
    version_id          TEXT,
    has_update          INTEGER NOT NULL DEFAULT 0 CHECK (has_update IN (0, 1)),
    update_version_id   TEXT,
    date_added          TEXT    NOT NULL,
    UNIQUE (server_id, file_path)
);

CREATE INDEX content_items_server_id ON content_items(server_id);
CREATE INDEX content_items_project_id ON content_items(project_id);

-- 8.9 blocks the delete dialog and must answer without asking Modrinth, so the
-- dependency list is written down beside the file at install time.
CREATE TABLE content_dependencies (
    content_id  TEXT NOT NULL REFERENCES content_items(id) ON DELETE CASCADE,
    project_id  TEXT NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN (
                    'required', 'optional', 'incompatible', 'embedded')),
    PRIMARY KEY (content_id, project_id)
);

CREATE INDEX content_dependencies_project_id ON content_dependencies(project_id);

-- 8.10–8.12. One linked modpack per server; the file list itself is in
-- content_items with source_kind = 'modrinth_modpack'.
CREATE TABLE server_modpacks (
    server_id          TEXT    PRIMARY KEY REFERENCES servers(id) ON DELETE CASCADE,
    source_kind        TEXT    NOT NULL CHECK (source_kind IN ('modrinth_modpack', 'local')),
    project_id         TEXT,
    version_id         TEXT,
    -- An uploaded pack has no Modrinth project to read a name off later.
    title              TEXT    NOT NULL,
    filename           TEXT,
    version_number     TEXT,
    date_published     TEXT,
    has_update         INTEGER NOT NULL DEFAULT 0 CHECK (has_update IN (0, 1)),
    update_version_id  TEXT,
    linked_at          TEXT    NOT NULL
);

-- 9.2: Minecraft rewrites server.properties from memory when it shuts down, so
-- an edit made while it runs would be gone by morning. Every change is kept here
-- and played in again after the process stops, before the next one starts. A row
-- with value NULL is a line to remove.
CREATE TABLE server_property_overrides (
    server_id   TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    key         TEXT NOT NULL CHECK (length(key) > 0),
    value       TEXT,
    queued_at   TEXT NOT NULL,
    PRIMARY KEY (server_id, key)
);


-- ------------------------------------------------------------- Prüfprotokoll
--
-- 11.9. Kept for 180 days, and it goes when its server goes. The actor carries
-- no foreign key on purpose: this is a record of what happened, and deleting a
-- panel user must not quietly rewrite who did what.

CREATE TABLE audit_log (
    id             TEXT PRIMARY KEY CHECK (length(id) = 26),
    server_id      TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
    actor_user_id  TEXT NOT NULL CHECK (length(actor_user_id) = 26),
    action         TEXT NOT NULL CHECK (action IN (
                       'server_created', 'server_reallocated', 'server_repaired',
                       'server_reset', 'server_started', 'server_stopped',
                       'server_restarted', 'server_killed', 'console_cleared',
                       'console_command_executed', 'changed_server_name', 'user_invited',
                       'user_invite_revoked', 'user_permission_modified', 'user_removed',
                       'addon_added', 'addon_uploaded', 'addon_disabled', 'addon_enabled',
                       'addon_deleted', 'addon_updated', 'modpack_changed',
                       'modpack_unlinked', 'port_allocation_added', 'port_allocation_removed',
                       'loader_version_edited', 'game_version_edited',
                       'server_properties_modified', 'startup_command_modified',
                       'java_runtime_modified', 'java_version_modified', 'file_uploaded',
                       'file_deleted', 'file_renamed', 'file_edited', 'backup_created',
                       'backup_renamed', 'backup_restored', 'backup_deleted')),
    metadata       TEXT CHECK (metadata IS NULL OR json_valid(metadata)),
    created_at     TEXT NOT NULL
);

CREATE INDEX audit_log_server_created_at ON audit_log(server_id, created_at DESC);
CREATE INDEX audit_log_created_at ON audit_log(created_at);


-- ------------------------------------------------------- Admin-Einstellungen
--
-- 12.10 gathers the four settings three area contracts assumed and none of them
-- defined. One row, written whole; the defaults below are the ones the contract
-- names (10.12, 8.8, 5.13, 17.8).

CREATE TABLE panel_settings (
    id                          INTEGER PRIMARY KEY CHECK (id = 1),
    -- The one thing the machine cannot work out for itself: a server binds
    -- 0.0.0.0 and there is no address per server. Guessing it would be wrong
    -- behind NAT and wrong behind a reverse proxy.
    public_address              TEXT,
    port_pool_from              INTEGER NOT NULL CHECK (port_pool_from BETWEEN 1024 AND 65535),
    port_pool_to                INTEGER NOT NULL CHECK (port_pool_to BETWEEN 1024 AND 65535),
    default_memory_mib          INTEGER NOT NULL CHECK (default_memory_mib >= 512),
    default_cpu_mode            TEXT    NOT NULL CHECK (default_cpu_mode IN ('cap', 'share')),
    default_cpu_cores           REAL    NOT NULL CHECK (default_cpu_cores > 0),
    default_pids_max            INTEGER NOT NULL CHECK (default_pids_max >= 64),
    max_upload_bytes            INTEGER NOT NULL CHECK (max_upload_bytes > 0),
    max_backups_per_server      INTEGER NOT NULL CHECK (max_backups_per_server BETWEEN 1 AND 50),
    external_services_enabled   INTEGER NOT NULL DEFAULT 1
                                        CHECK (external_services_enabled IN (0, 1)),
    max_concurrent_operations   INTEGER NOT NULL CHECK (max_concurrent_operations >= 1),
    stop_grace_seconds          INTEGER NOT NULL CHECK (stop_grace_seconds > 0),
    updated_at                  TEXT    NOT NULL,
    CHECK (port_pool_from <= port_pool_to)
);

INSERT INTO panel_settings (
    id, public_address, port_pool_from, port_pool_to,
    default_memory_mib, default_cpu_mode, default_cpu_cores, default_pids_max,
    max_upload_bytes, max_backups_per_server, external_services_enabled,
    max_concurrent_operations, stop_grace_seconds, updated_at
) VALUES (
    1, NULL, 25565, 25700,
    4096, 'cap', 2.0, 512,
    4294967296, 10, 1,
    2, 60, '1970-01-01T00:00:00Z'
);


-- --------------------------------------------------------- Modrinth-Speicher
--
-- 8.16. Four tables, and the last two are not decoration: title, icon, owner and
-- the modpack figures appear in no version answer at all. Without them, opening
-- the content page of a server with 40 mods costs 41 calls to Modrinth.

CREATE TABLE modrinth_project (
    project_id    TEXT PRIMARY KEY,
    slug          TEXT,
    title         TEXT NOT NULL,
    description   TEXT,
    icon_url      TEXT,
    project_type  TEXT,
    downloads     INTEGER,
    followers     INTEGER,
    team          TEXT,
    environment   TEXT,
    fetched_at    TEXT NOT NULL,
    expires_at    TEXT NOT NULL
);

CREATE TABLE modrinth_project_versions (
    project_id  TEXT PRIMARY KEY,
    payload     TEXT NOT NULL CHECK (json_valid(payload)),
    -- Revalidated with If-None-Match once the six hours are up.
    etag        TEXT,
    fetched_at  TEXT NOT NULL,
    expires_at  TEXT NOT NULL
);

CREATE TABLE modrinth_version (
    version_id  TEXT PRIMARY KEY,
    project_id  TEXT NOT NULL,
    payload     TEXT NOT NULL CHECK (json_valid(payload)),
    fetched_at  TEXT NOT NULL,
    expires_at  TEXT NOT NULL
);

CREATE INDEX modrinth_version_project_id ON modrinth_version(project_id);

CREATE TABLE modrinth_project_owner (
    team_id     TEXT PRIMARY KEY,
    owner_id    TEXT NOT NULL,
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN ('user', 'organization')),
    avatar_url  TEXT,
    fetched_at  TEXT NOT NULL,
    expires_at  TEXT NOT NULL
);
