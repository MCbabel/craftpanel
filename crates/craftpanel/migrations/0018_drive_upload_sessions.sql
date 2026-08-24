-- Section 22: an upload that outlives a restart of the panel.
--
-- Google holds a resumable upload session open for a week ("A resumable session
-- URI expires after one week"), and this panel is expressly built to be
-- restarted while it runs — the supervisors survive it, so a restart is an
-- ordinary event and not an accident. Until now it threw half an upload away:
-- the session address lived in a local variable of the sending task and in
-- nothing else, so the next attempt started again at byte zero. On a two-gigabyte
-- world over a domestic line that is the difference between a backup and no
-- backup.
--
-- What is deliberately NOT stored is how far the upload had got. Google's own
-- instruction is to ask: a status query answers 308 with a Range header, "and
-- don't assume that the server received all bytes sent in the previous request".
-- A remembered offset would be a second opinion about a fact only Google holds,
-- and the one thing worse than restarting an upload is stitching a file out of
-- two different halves. So the resume always asks first and believes the answer.


-- ------------------------------------------------------- Wo die Adresse liegt
--
-- The session address is not in this table and must never be. Whoever has it can
-- write into that user's Google Drive; for Cloud Storage the same protocol says
-- so outright ("This session URI acts as an authentication token... requests
-- that use it don't need to be signed"), for Drive it is not documented either
-- way, and the upload_id sits in the URL. It is treated the way 0012 treats the
-- refresh token: <data_dir>/drive/<user_id>/sessions/<backup_id>, mode 0600 in a
-- 0700 directory, written to a .part file and renamed into place. That also gives
-- one act of forgetting instead of two — disconnecting an account already removes
-- <data_dir>/drive/<user_id> whole, and every open session goes with it.
--
-- This table is the authority over the pair: a row without its file starts the
-- upload over, a file without its row is swept off the disk at the next start.
-- Neither is a failure, because both end in the same safe place.


CREATE TABLE drive_uploads (
    -- One session per backup, and the row dies with the backup. A backup that
    -- was deleted, pruned by keep_last or thrown away as a half-finished create
    -- takes its session with it, which is right: the session refers to an
    -- archive that no longer exists.
    backup_id        TEXT    PRIMARY KEY REFERENCES backups(id) ON DELETE CASCADE,

    -- Whose Drive it writes into. Not derivable cheaply enough to leave out —
    -- backups -> servers -> owner_id is two joins, and this column is also what
    -- says which of the per-user key directories holds the address. If the user
    -- is gone, so is the Drive the session pointed at.
    user_id          TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,

    -- The size the session was opened for: it went to Google as
    -- X-Upload-Content-Length and it goes back to Google in every
    -- "Content-Range: bytes */<total>". A session is bound to one length, so a
    -- resume that finds a different one on disk cannot use it.
    --
    -- It is also the first third of the fingerprint below, because the length of
    -- the archive on disk and the length Google was promised are the same number.
    total_bytes      INTEGER NOT NULL CHECK (total_bytes > 0),

    -- The other two thirds. THIS IS THE DANGEROUS CASE and the reason these two
    -- columns exist: the archive of a backup is written to one path,
    -- <data_dir>/backups/<server>/<backup>.tar.zst, and pressing Retry on a
    -- failed backup packs a *new* archive to that same path under the same
    -- backup id. Resuming an old session into a new archive would put the first
    -- half of Monday's world and the second half of Tuesday's into one file that
    -- Google then reports as a complete, healthy backup. Nothing downstream would
    -- catch it except the checksum at the very end, and nothing at all would
    -- catch it if Google named no checksum — which nothing obliges it to do: the
    -- last chunk is answered "along with any metadata associated with the
    -- resource" and no field is named
    -- (developers.google.com/workspace/drive/api/guides/manage-uploads).
    --
    -- Drive documents no defence of its own: no Content-MD5 header on upload, no
    -- per-chunk check, no way to cancel a session. The client has to be the one
    -- that refuses, so the resume compares all three numbers and starts over on
    -- any difference. Modification time is kept in nanoseconds rather than as an
    -- RFC 3339 timestamp because Timestamp rounds to the second and two packs of
    -- the same world land well inside one second of each other.
    archive_mtime_ns INTEGER NOT NULL,
    archive_inode    INTEGER NOT NULL,

    -- When Google handed the address out. The week is counted from here, which is
    -- the safe reading of a documentation that says both "expires after one week"
    -- and "expire after one week of inactivity" and never resolves the two. Our
    -- clock only prunes; a session is dead when Google says 404, not when we say
    -- so. Nothing here is ever presented to the user as "still good" on the
    -- strength of this column alone.
    opened_at        TEXT    NOT NULL,

    updated_at       TEXT    NOT NULL
);

-- Sweeping is per account (disconnecting one, or letting go of everything that
-- account left open), so the lookup that is not the primary key is by user.
CREATE INDEX drive_uploads_user ON drive_uploads(user_id);
