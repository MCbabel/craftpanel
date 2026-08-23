-- Section 5.9 gains an eighth phase: `installing_java`.
--
-- A server that is being set up, and a server whose start found no runtime,
-- both fetch a JRE (section 23). That fetch is 40 to 60 MB with a progress bar
-- of its own, and it is neither `installing_loader` nor `writing_config`. The
-- interface reads `phase` to name what is happening, so the name has to exist
-- here as well: 0002 wrote the seven values into a CHECK, and a CHECK is the one
-- thing SQLite cannot alter in place.
--
-- Hence the table is built anew. It is the whole of 0002's `operations` plus
-- 0004's two columns, with one more word in one CHECK; nothing else changes,
-- and every row moves across.
--
-- The order is chosen so that this works with foreign keys switched on, which
-- they are (db.rs), and inside the transaction sqlx wraps every migration in --
-- `PRAGMA foreign_keys = OFF` would be ignored there:
--
--   1. the rename carries the self-reference of parent_operation_id along with
--      it, so the old table stays consistent while it still holds every row;
--   2. the rows come over with that column empty and it is filled afterwards,
--      so a safety copy never arrives before the restore it belongs to;
--   3. the old table is dropped when nothing points at it any more. Its indexes
--      go with it, so all five are written again below.

ALTER TABLE operations RENAME TO operations_before_java;

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
                                     'running_installer', 'installing_java', 'installing_pack',
                                     'addons', 'writing_config')),
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
    target_id            TEXT    REFERENCES backups(id) ON DELETE SET NULL,
    parent_operation_id  TEXT    REFERENCES operations(id) ON DELETE SET NULL,
    started_by           TEXT    REFERENCES users(id) ON DELETE SET NULL,
    input                TEXT    CHECK (input IS NULL OR json_valid(input)),
    payload              TEXT    NOT NULL DEFAULT 'none'
                                 CHECK (payload IN ('none', 'expected', 'delivered')),
    applied_at           TEXT,
    created_at           TEXT    NOT NULL,
    started_at           TEXT,
    finished_at          TEXT,
    dismissed_at         TEXT,
    cancel_requested     INTEGER NOT NULL DEFAULT 0 CHECK (cancel_requested IN (0, 1)),
    progressed_at        TEXT,
    CHECK ((error_code IS NULL) = (error_step IS NULL)
           AND (error_code IS NULL) = (error_message IS NULL))
);

INSERT INTO operations (
    id, server_id, kind, state, phase, progress, message, src, bytes_processed,
    files_processed, current_file, error_code, error_message, error_step, cancellable,
    target_id, parent_operation_id, started_by, input, payload, applied_at, created_at,
    started_at, finished_at, dismissed_at, cancel_requested, progressed_at
)
SELECT
    id, server_id, kind, state, phase, progress, message, src, bytes_processed,
    files_processed, current_file, error_code, error_message, error_step, cancellable,
    target_id, NULL, started_by, input, payload, applied_at, created_at,
    started_at, finished_at, dismissed_at, cancel_requested, progressed_at
FROM operations_before_java;

UPDATE operations
   SET parent_operation_id = (
       SELECT parent_operation_id FROM operations_before_java AS before
        WHERE before.id = operations.id
   );

DROP TABLE operations_before_java;

CREATE INDEX operations_server_id ON operations(server_id, id DESC);
CREATE INDEX operations_open ON operations(server_id) WHERE state IN ('queued', 'ongoing');
CREATE INDEX operations_target_id ON operations(target_id);

CREATE UNIQUE INDEX operations_one_open_backup_create
    ON operations(server_id) WHERE kind = 'backup_create' AND state IN ('queued', 'ongoing');
CREATE UNIQUE INDEX operations_one_open_backup_restore
    ON operations(server_id) WHERE kind = 'backup_restore' AND state IN ('queued', 'ongoing');
