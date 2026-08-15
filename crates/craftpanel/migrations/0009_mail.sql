-- Section 19: the panel can send mail through Resend. Two tables, and neither of
-- them holds the API key.
--
-- The key lives in <data_dir>/mail/api_key with mode 0600 in a 0700 directory,
-- exactly as playit's key does (0008_playit_per_user.sql), so a copy of this
-- database — for a bug report, for a backup — still carries no way in to
-- somebody else's service. Not one character of it is written here, not even a
-- hint like `re_…AB12`: with a single key in the whole panel a hint buys nothing
-- and would be a piece of the secret in every copy.


-- ------------------------------------------------------------ Einstellungen

CREATE TABLE mail_settings (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    provider      TEXT    NOT NULL DEFAULT 'resend' CHECK (provider IN ('resend')),
    -- Resend takes no other sender until a domain is verified, and then only to
    -- the address the Resend account was opened with (19.1). This default is
    -- therefore the one value with which the test button works on day one.
    from_address  TEXT    NOT NULL DEFAULT 'onboarding@resend.dev',
    from_name     TEXT    NOT NULL DEFAULT 'mcpanel',
    reply_to      TEXT,
    -- The address of the *panel*, with scheme, e.g. https://panel.example.com.
    -- Deliberately not panel_settings.public_address: that one becomes
    -- Server.net.ip (12.10, servers/manager.rs:1664) — the address players
    -- connect to, usually a bare name or an IP, and turning it into a URL would
    -- guess `http` where a reverse proxy wants `https`. Empty means no mail with
    -- a link goes out at all (19.2, mail_no_link_base).
    link_base     TEXT,
    -- Our own brake, not Resend's. The free tier gives 100 mails a day; a bug
    -- that burns them would take real password resets down with it.
    daily_limit   INTEGER NOT NULL DEFAULT 100 CHECK (daily_limit >= 0),
    -- When the key file was written. The only thing about the key in this
    -- database.
    key_set_at    TEXT,
    last_test_at  TEXT,
    last_error    TEXT,
    last_error_at TEXT,
    updated_at    TEXT    NOT NULL,
    CHECK ((last_error IS NULL) = (last_error_at IS NULL))
);

INSERT INTO mail_settings (id, updated_at) VALUES (1, '1970-01-01T00:00:00Z');


-- ------------------------------------------------------------- Warteschlange
--
-- 19.6: queue, rate counter and log in one table. Counting with
-- `SELECT count(*) … WHERE created_at > …` survives a restart, which the login
-- brake (auth/brake.rs:5-7) says of itself that it does not.

CREATE TABLE mail_outbox (
    -- ULID, and at the same time the Idempotency-Key sent to Resend: Crockford
    -- base32 is inside Resend's alphabet and far below its 256 characters. A
    -- retry after a crash therefore carries the same key as the first attempt,
    -- and Resend answers with the same id instead of sending a second mail.
    id              TEXT    PRIMARY KEY CHECK (length(id) = 26),
    kind            TEXT    NOT NULL CHECK (kind IN ('verify_email',
                                                     'address_already_registered',
                                                     'account_awaiting_review',
                                                     'account_approved',
                                                     'account_rejected',
                                                     'reset_password',
                                                     'password_changed',
                                                     'test')),
    -- Delete the account and its unsent post goes with it: a reset mail for a
    -- deleted account must not go out, and the address of a deleted account
    -- should not lie around. Null for the test mail and for mail to an address
    -- that has no account (yet).
    user_id         TEXT    REFERENCES users(id) ON DELETE CASCADE,
    to_address      TEXT    NOT NULL,
    subject         TEXT    NOT NULL,
    -- The rendered body. Set to NULL once delivery succeeded, because it holds
    -- the clear-text link while the token itself is only a hash in its own table
    -- (auth/session.rs:3-6). After delivery this database holds no secret again;
    -- only mail in flight carries one, and that is the shortest window there is.
    html            TEXT,
    text            TEXT,
    state           TEXT    NOT NULL CHECK (state IN ('queued', 'sending', 'sent', 'failed')),
    attempts        INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at TEXT,
    -- Resend's own mail id. The only receipt we get from there; it says accepted,
    -- not arrived (19.13).
    provider_id     TEXT,
    last_error      TEXT,
    created_at      TEXT    NOT NULL,
    sent_at         TEXT,
    CHECK ((state = 'sent') = (sent_at IS NOT NULL))
);

CREATE INDEX mail_outbox_due ON mail_outbox(state, next_attempt_at);
CREATE INDEX mail_outbox_recent ON mail_outbox(created_at DESC);
-- The three brakes of 19.6 count over this one.
CREATE INDEX mail_outbox_rate ON mail_outbox(to_address, kind, created_at);
