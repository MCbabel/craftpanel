-- Section 22: what this panel has handed to Google today, counted by the panel.
--
-- Google allows one account 750 GB of upload a day: users "can only upload
-- 750 GB per day between My Drive and all shared drives", and whoever reaches
-- that "can't upload or copy additional files until 24 hours have passed"
-- (developers.google.com/workspace/drive/api/guides/limits). The refusal that
-- arrives when it happens is a 403 "User rate limit exceeded" or a 429 — the exact
-- reason string is not documented — and both of those are precisely what this
-- panel's classifier treats as a bad moment worth waiting out. So the ceiling
-- arrives dressed as something to retry, and a retry cannot help for a day.
--
-- The defence has to be our own count, because Google offers none: about.get
-- reports storageQuota (how full the Drive is) and says nothing at all about how
-- much has gone up today. There is no endpoint for it.
--
-- Which 750 GB is not written down either. Google says "GB" on every page and
-- never says whether that is 1000^3 or 1024^3 bytes, so day::CEILING takes the
-- decimal reading, 750,000,000,000: it is the smaller of the two, and only the
-- smaller one keeps the promise below under both readings — the binary one would
-- count 55,306,368,000 bytes past Google's limit if the decimal reading is the
-- right one, and then this count would stop later than Google, not earlier.


CREATE TABLE drive_daily_uploads (
    user_id    TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,

    -- The calendar day in UTC, "YYYY-MM-DD". Google's own window is a rolling 24
    -- hours from whenever the account went over, which is not a thing a table can
    -- hold and not a thing Google publishes the start of. A UTC day is the honest
    -- approximation: it can only make the panel stop *earlier* than Google would,
    -- never later, because the count never carries yesterday's bytes forward.
    day        TEXT    NOT NULL,

    -- Bytes Google acknowledged, not bytes offered. A chunk that was sent and
    -- refused went over the wire and is not counted here, so this number is a
    -- floor under Google's own meter and never an overstatement of what is left.
    --
    -- It is a floor for a second reason as well, and this one cannot be fixed
    -- from here: the 750 GB belong to the Google account, not to this panel. If
    -- its owner uploads to the same Drive from his phone, that goes on Google's
    -- meter and not on ours. So this count catches the case the panel can cause
    -- on its own — a night of scheduled backups — and the other case still
    -- arrives as the 403 it always did.
    bytes      INTEGER NOT NULL DEFAULT 0 CHECK (bytes >= 0),

    updated_at TEXT    NOT NULL,

    PRIMARY KEY (user_id, day)
);

-- Old days are swept, not kept: the count exists to answer "how much room is
-- left today" and nothing else. It is deliberately not a history of anybody's
-- backups — backups already have their own rows with their own sizes.
CREATE INDEX drive_daily_uploads_day ON drive_daily_uploads(day);
