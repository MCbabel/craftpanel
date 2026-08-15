CREATE TABLE users (
    id             TEXT    PRIMARY KEY,
    username       TEXT    NOT NULL UNIQUE,
    password_hash  TEXT    NOT NULL,
    role           TEXT    NOT NULL CHECK (role IN ('admin', 'user')),
    system_uid     INTEGER,
    system_state   TEXT    NOT NULL DEFAULT 'pending'
                           CHECK (system_state IN ('pending', 'ready', 'failed')),
    memory_limit   INTEGER,
    cpu_limit      INTEGER,
    pids_limit     INTEGER,
    created_at     TEXT    NOT NULL,
    updated_at     TEXT    NOT NULL
);

CREATE TABLE sessions (
    id          TEXT    PRIMARY KEY,
    user_id     TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at  TEXT    NOT NULL,
    expires_at  TEXT    NOT NULL,
    last_seen   TEXT    NOT NULL,
    user_agent  TEXT
);

CREATE INDEX sessions_user_id ON sessions(user_id);
CREATE INDEX sessions_expires_at ON sessions(expires_at);
