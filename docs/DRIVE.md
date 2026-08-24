# Google Drive

Status: 2026-08-24. Design and build record for section **22** of the contract: backups into the
user's Google Drive. **A server's backups sit in its owner's storage, not on the operator's disk.**

The shape is playit's (`docs/PLAYIT.md`, contract 18): **one account per panel user.** The operator
sets up a Google project, every user connects their own Google account, and the panel provides none
of its own: the consent happens in the browser of the person the account belongs to.

Every statement about Google carries its source (fetched 2026-08-13/14). Every statement about what
is already there carries `file:line`. The numbering `n` matches contract section 22.`n`.

---

## 0. The hard obstacle first: there is no redirect here

The chain of findings, in this order — it decides the whole area:

1. **A LAN address is inadmissible as a redirect target on two counts.** Google: "Redirect URIs must
   use the HTTPS scheme, not plain HTTP. Localhost URIs … are exempt from this rule." and "Hosts
   cannot be raw IP addresses. Localhost IP addresses are exempted from this rule."
   (developers.google.com/identity/protocols/oauth2/web-server). `http://192.168.1.10:8099/…` fails
   on both, and that is exactly what this panel looks like in most setups.
2. **The copy-it-down detour is dead.** "The manual copy/paste option, also referred to as an out of
   band (OOB) redirect method, is no longer supported"
   (developers.google.com/identity/protocols/oauth2/native-app).
3. **The detour via `127.0.0.1` does not carry here.** It presumes a listener on the machine in
   whose browser consent is given. The user's browser sits on their PC, the panel on the server; a
   listener on the panel machine is never reached by that browser.
4. **The device flow covers exactly our area.** "The OAuth 2.0 flow for devices is supported only
   for the following scopes", and the list contains `https://www.googleapis.com/auth/drive.file`
   (developers.google.com/identity/protocols/oauth2/limited-input-device). The same page: "Note that
   refresh tokens are always returned for devices."

**Decided: device flow, a single path, no alternative.** No domain, no TLS, no redirect URI, no
second flow for "later, with a domain". That is why this area stays small, and it is the answer to
the question whether it can run without the operator doing anything: it can, as soon as they have
done five steps in the Google console, and the admin page names them one by one.

The mechanics, word for word from the same page and in the code in one place
(`crates/craftpanel/src/drive/oauth.rs`):

* `POST https://oauth2.googleapis.com/device/code` with `client_id` + `scope`. Response:
  `device_code`, `user_code`, **`verification_url`** (not `verification_uri`, as RFC 8628 calls
  it), `expires_in`, `interval`.
* `POST https://oauth2.googleapis.com/token` with `grant_type=urn:ietf:params:oauth:grant-type:device_code`.
* Polling errors: `authorization_pending` (428), `slow_down` (403), `access_denied` (403),
  `expired_token`, and the five from their section "Other errors": `admin_policy_enforced` (400),
  `invalid_client` (401), `invalid_grant` (400), `unsupported_grant_type` (400), `org_internal`
  (403), plus `rate_limit_exceeded` (403) on `/device/code`, which carries its word in the field
  **`error_code`**. Every one of these names has a sentence and a file in `drive/testdata/`; the
  table is in contract 22.5 and in the code in `oauth.rs::ending`.

Both field names are read (`oauth.rs`, `DeviceAnswer`), so that Google moving closer to the standard
is not an outage. A test records that their own name is the one that gets read:
`googles_own_field_name_is_the_one_that_is_read`.

---

## 1. The scope: `drive.file`, and nothing above it

`drive.file` is entirely sufficient: create a folder, files inside it, read, change and delete our
own files, and `about.get` for the storage state
(developers.google.com/workspace/drive/api/reference/rest/v3/about/get).

`drive.appdata` would be permitted too, but is **rejected**: the hidden app folder cannot be found
by the user, and the promise is "the backup sits in *your* Drive" — visible, clickable, downloadable
by yourself. `drive` and `drive.readonly` are "restricted" and would pull a review process with a
security assessment along with them.

The double in the test insists on it: `device_code` checks that the scope requested is `drive.file`
and nothing more (`drive/harness.rs`, `device_code`).

---

## 2. What the operator sets up — and what "not set up" means

Five steps, and the admin page lists them with their addresses (`web/src/pages/admin/Drive.vue`):
create a project → **enable the Drive API** (without it every call is `403 accessNotConfigured`) →
fill in the consent screen, audience *External*, scope `…/auth/drive.file` →
**"Publish app" → In production** → create a client, type **"TVs and Limited Input devices"**.

**Step four is mandatory and has a warning of its own.** "A Google Cloud Platform project with an
OAuth consent screen configured for an external user type and a publishing status of *Testing* is
issued a refresh token expiring in **7 days**, unless the only OAuth scopes requested are a subset
of name, email address, and user profile" (developers.google.com/identity/protocols/oauth2, under
`cloud_lock`; `drive.file` is none of those three). On "Testing" every connection therefore breaks
after a week — silently, and exactly when somebody needs a backup.

**The panel cannot query this state**; Google offers no interface for it. It can only say it and
guess: if a refresh fails with `invalid_grant` on a token younger than ten days, `last_error` says
word for word that this looks like a consent screen on *Testing* (`oauth.rs`,
`looks_like_a_testing_project` and `TESTING_HINT`; the test for it is
`a_withdrawn_connection_is_written_down_and_the_key_file_stays`).

The age of the connection has **no column**: it is the mtime of the token file (`keys.rs`,
`token_written_at`). The same measurement without a migration — and a token overwritten by a fresh
connection is thereby rightly young again.

Publishing needs no **additional** verification and no security assessment, because `drive.file`
stands in Google's table of "Non-sensitive scopes", and those "only require basic OAuth App
Verification" (developers.google.com/workspace/drive/api/guides/api-specific-auth). It is
"Sensitive and restricted scopes" that "require additional verification and security assessments"
(developers.google.com/workspace/guides/configure-oauth-consent). So the basic check stays — it is
not nothing — and the review that takes weeks does not apply.

**Nothing set up is a normal state, not an error.** Measured in the test
`a_panel_with_nothing_set_up_answers_and_calls_nobody`: `GET /api/v1/drive` answers `200` with
`panel_configured: false`, every attempt to connect `409 drive_not_configured`, and the double
counts **zero** calls. Half set up does not count as set up: a client id without a secret file
would fail on the first call (`an_id_without_a_secret_is_not_a_setup`).

---

## 3. Where the secrets live

The model is `migrations/0008_playit_per_user.sql`, word for word: "a copy of this file — for a bug
report, for a backup — still carries no way in to somebody else's service."

| What | Where | Mode |
|---|---|---|
| client secret (panel-wide) | `<data_dir>/drive/client_secret` | 0600 in 0700 |
| refresh token per user | `<data_dir>/drive/<user_id>/refresh_token` | 0600 in 0700 |
| session address of an upload in flight | `<data_dir>/drive/<user_id>/sessions/<backup_id>` | 0600 in 0700 |
| access token | **in memory only**, `RwLock<Option<Access>>` with a deadline | — |
| `device_code` of a running operation | **only in the polling loop** | — |
| client id | database, `drive_settings.client_id`, not a secret | — |

The `Secret` newtype (`oauth.rs`) gives only `Secret(hidden)` as `Debug`. Outside the tests
`Secret::expose` is called in five places, and each one is a place where the plain text has to go
out or onto the disk: the token request and the refresh (`oauth.rs`, `poll` and `refresh`), the
revocation (`oauth.rs`, `revoke`), the key file (`keys.rs`, `write`), and the upload session
address, which is read back out of its file to be used as a URL (`mod.rs`, `carry_on`). Everything
else goes through `Access::expose`, the same door for the access token that every call in
`files.rs` and `upload.rs` puts into `Authorization`.

**The `device_code` is not in the database.** It is the voucher the token can be fetched with;
playit's `claim_code` is in there only because the *user* visits it. Consequence: a panel restart
throws away a running operation (`Drive::pick_up` clears the row up), the user presses again. That is
cheaper than the alternative.

**The `user_code` is not a harmless field either.** Confirm somebody else's and you hang *your*
Drive on *somebody else's* panel account; from then on that account's server backups flow into the
stranger's Drive. That is a data leak, not merely bad manners. Two tests check every admin response
for it: `the_overview_carries_no_user_code_at_all` (the row) and `an_admin_never_sees_a_user_code`
(the finished JSON response).

**When the token is withdrawn** — the user revokes it, six months unused, or the 7-day trap — the
refresh answers `invalid_grant`. Then: `drive_accounts.state = 'revoked'`, `last_error` in plain
words, access token in memory gone, **and the key file stays.** The model is
`PlayitStatus.configured` (`playit/mod.rs`): a key is here, and that says nothing about whether it
still works. The state belongs in the column, not in the presence or absence of a file.

The hook for the mail that 22.17 names as the fourth net sits in `Account::mark_revoked` and today
writes a `warn!` line.

**`invalid_grant` does not mean the same thing everywhere.** The parser turns it into `Revoked`
everywhere, but on a **poll** of the device operation it means no withdrawn connection at all, only
that this code is used up (22.4), and the whole answer to that is a new code. So the polling loop has
to tell the case apart itself (`drive/oauth.rs`, `ending`).

**And `revoked` is the only state that holds up a backup run.** Because the key file stays, the mere
presence of a file kept saying "connected" while every upload failed, so the door in front of a run
asks for the file **and** the column. `error` explicitly does **not** belong there: a sweep that
failed to reach Google once is no reason to refuse a backup that could succeed
(`drive/mod.rs`, `Drive::guard_backup` and `usable`).

**The panel-wide Drive settings live in a row of their own, not in `panel_settings`.** That row is
written as a whole by 12.11; a second area writing into it would put a second hand on
`auth/settings.rs`, `api/admin.rs` and the settings page (`drive/store.rs`, `Settings`).

---

## 4. The flow of a backup: build → upload → delete locally

**The first reason is compelling and comes from what exists, not from taste.**
`quiesce::Held::take` switches `save-off`, `Drop` switches it back (`backups/quiesce.rs`), and the
whole risk of that area is this: a server left with saving switched off looks perfectly healthy and
loses everything since the last flush the moment it crashes. **A streaming upload would hold
`save-off` for the entire upload**: at 2 GB and 10 Mbit/s that is half an hour without saving.

That is why the upload sits **outside** the bracket: `pack()` holds it and gives it back,
`deliver()` uploads afterwards (`backups/mod.rs`, `run_create`). The code carries no comment saying
so — this file is where the reasons live — and a test nails it down:
`saving_is_switched_back_on_before_a_single_byte_goes_to_google`. It stalls the first chunk for
300 ms and checks that **the last** command sent to the server at that moment was `save-on`, not
merely that `save-on` occurred at some point. Counter-check run: a second `quiesce` bracket put
around the upload, and the test fails with
`["save-off", "save-all flush", "save-on", "save-off", "save-all flush"]`.

Two further reasons say the same. A `tar`+zstd stream is not reproducible (`archive.rs` has its own
tests for "the file shrinks while being read"), but resuming demands the same bytes in the same
place; and the finished file knows its length, so `X-Upload-Content-Length` is set and every
`Content-Range` is exact.

**The run, one operation, three parts:** pack as before (progress 0 → 0.5), upload (0.5 → 1.0,
`watch_between`), then **write the row first, delete the local file second**: the other way round, a
crash in between would be a file without a row *and* without a local copy (the same ordering
reasoning as `forget()`). No new `OperationPhase` value.

**The upload** (developers.google.com/workspace/drive/api/guides/manage-uploads), in `upload.rs`:

* `POST …/upload/drive/v3/files?uploadType=resumable`, metadata in the body, session URI in the
  `Location` header, valid for a week.
* Chunks as `PUT`, **multiples of 256 KiB**, chosen **8 MiB** (32 of them).
* `308` means carry on, and **the `Range` header of the response says what arrived**, not our own
  count. The response may carry a **new `Location`**, and then that one applies.
* State after an interruption: empty `PUT` with `Content-Range: bytes */<total>`.
* `404` means "session expired", and the **running run** starts over: the dead address is let go,
  a session of its own is opened and the archive goes up from the front, once. Google's own words
  are "restart the upload", and postponing that to the next press was our addition, not theirs. A
  Google that forgets the second session too ends the run as `drive_session_expired` — which no
  longer reads as `drive_file_missing`, the code for a file that is not there at all
  (`a_session_google_forgets_in_mid_flight_is_opened_again_in_the_same_run` and
  `a_google_that_forgets_every_session_ends_the_run_saying_the_session_ran_out`).
* `5xx`/`429`: the backoff of `drive/retry.rs`, counted **per chunk** for how many tries a call
  gets and **per run** for how long the whole thing may wait. Per chunk, because a run over 250
  chunks that gave up somewhere after a few bad moments would never get through on a bad line; per
  run, because seven tries and 65 s of backoff *per call* over 250 chunks is four and a half hours
  of a run that is doing nothing. `Waiting::within` hands every call of one upload the same
  `Spent`, and when the run's budget is gone the run ends as `drive_throttled` with its session
  kept (`the_budget_for_the_waiting_covers_the_run_and_not_only_the_single_call`).
* **The loop needs a counter of its own for "`308`, but nothing arrived".** Such a `308` is not an
  error, so the retry never sees it; without this counter the loop would run **forever** against a
  Google that keeps answering "I have nothing" (`drive/upload.rs`, the `fruitless` counter in
  `send`).

**Waiting it out, in one place for every call** (`drive/retry.rs`). Google names exponential
backoff as the answer to `429` and `5xx`
(developers.google.com/workspace/drive/api/guides/handle-errors), and the numbers on the limits
page: wait `min((2^n) + random_number_milliseconds, maximum_backoff)`, "maximum_backoff is
typically 32 or 64 seconds". So: 1 s, 2 s, 4 s … up to 64 s, plus up to a second of chance —
**the chance matters**, because without it every account on the machine walks back into the same
wall in step. `Waiting::keep_trying` wraps every call that goes out to Google, and what may be
repeated at all is decided by `DriveError::is_worth_repeating` alone — the classifier that was
already there, not a second one.

Four things it does that a bare retry loop does not:

* **`Retry-After` is an order, not a suggestion.** Google does not document that Drive ever sends
  it — the header appears on none of the four pages (handle-errors, limits, manage-uploads, the
  OAuth ones) — so the schedule works without it, and when it does arrive it is a floor under our
  own wait, capped at ten minutes so that a strange header cannot hang a run.
* **Two ceilings, not one:** a number of tries *and* a budget for the whole wait
  (`Backoff::PATIENT`: seven tries, five minutes; `Backoff::BRIEF` for anything a page is waiting
  on: three tries, ten seconds). The 750 GB daily upload limit is the reason. Google does not
  document which `reason` it comes back as, and the limits page names only `403 User rate limit
  exceeded` and `429` — which our classifier reads as "worth repeating". Without a budget a run
  would circle for the 24 hours that limit lasts and tell nobody.
* **A cancel takes hold during the wait**, not after it: `Waiting` carries the run's
  `archive::Progress` and sleeps in slices.
* **A bar that stands still says why.** The wait writes a sentence into `Progress`, `watch_between`
  carries it into the operation's `message`, and `Backups.vue` prints it under the bar: "sending an
  archive up: Google is turning us away, so the next try (3 of 7) is in 4 seconds".

Where it is used: the chunk `PUT`s and the status query, opening the session, the folder lookup and
its creation, `files.get`, `files.list`, `about.get`, `files.delete`, the start of a download, and
the **token refresh** — a refresh that meets a `503` used to end an upload of two gigabytes. What is
deliberately left alone: the device-flow calls `oauth::begin` and `oauth::poll`, because Google
prescribes their rhythm itself (`interval`, `slow_down`) and a second backoff on top of it would
fight the first; `oauth::revoke`, which is best-effort on the way out; and a download that has
already begun to write bytes — a repeat would fetch what is already here a second time, so that
one carries on with a `Range` request at the next attempt (section 5a).

The test that proves something: `a_multi_chunk_upload_survives_a_503_and_a_short_acknowledgement`
sends 17 MiB of incompressible noise (three chunks), makes the double answer `503` on the second
chunk and, after the first, **acknowledge less than arrived**. Expected: the run ends successfully
and the bytes at the target are byte for byte the archive. Counter-check run: the `Range` evaluation
replaced by "everything we sent has arrived" — the double catches it with "a chunk arrived at the
wrong offset — the resume is broken, and the file would have a hole in it", and the run fails.

**Three limits must not get into the backoff cycle**, otherwise the upload hammers for a minute
against a wall that only opens tomorrow: `storageQuotaExceeded` (per Google explicitly not to be
retried), the 750 GB of upload per user account per day, and 5 TB per file.
`DriveError::is_worth_repeating` decides that in one place, and the difference hangs solely on
`errors[0].reason`: a full Drive and throttling are **both** HTTP 403 (`http.rs`,
`a_full_drive_and_a_rate_limit_both_arrive_as_403`). Measured in a run:
`a_full_drive_fails_the_run_at_once_and_says_so` checks that exactly **one** chunk was attempted.

**Cancelling** checks the same `archive::Progress::is_cancelled` between two chunks. The consolation:
as long as a resumable session is not finished, **no file appears in the user's Drive**. The double
reproduces that, and the same test shows that after a failed run only the folder is there.

**For 5.12 (restart) there was once "nothing to add", and that sentence is now wrong.** It stood
here and in contract 22.15 until `0018_drive_upload_sessions.sql`, and the code left it behind:
`recover()` keeps a half archive that has a live matching session, the session address survives in
`<data_dir>/drive/<user_id>/sessions/<backup_id>`, and the Retry button sends the rest. Section 4b
below is the whole reasoning; this paragraph stands here so that nobody reads the old sentence
again.

File name: `<server>--<backup>--<created_at>.tar.zst` through the same `slug()` as a download
(`api/backups.rs`), plus `appProperties { panel, server_id, backup_id }`.

**What went up is held against what left, and what cannot be held against anything says so.** The
answer to the last chunk carries the metadata of the file Google now holds;
`fields=id,size,md5Checksum,sha256Checksum` on the initiating request asks for all of it, because
the default set of `files.create` is **not documented** and nobody may count on a checksum coming
along unasked. Google offers `md5Checksum`, `sha1Checksum` and `sha256Checksum`, marks none of them
as deprecated and gives each a condition instead of a promise — the MD5 "is only applicable to
files with binary content in Google Drive", the other two are there "if available"
(developers.google.com/workspace/drive/api/reference/rest/v3/files) — so asking for the second one
costs a longer query string and nothing else. Our own md5 **and** sha256 grow beside the upload:
every byte Google acknowledges is fed to both digests as it is read for sending, so a 2 GB archive
is still read exactly once. **The strongest checksum Google names is the one that
decides** — sha256 before md5 — because a checksum is only worth the collisions it refuses, and the
second pass of arithmetic runs over bytes that are already in the cache. Then three outcomes, and
only the first is a confirmed backup:

* Google names a checksum, it is equal, and the size is equal → the run succeeds and the md5 of the
  archive is kept in `backups.drive_md5`;
* a checksum that differs, or a size other than the one that was sent → the file is **deleted in the
  Drive** and the run fails with `drive_checksum_mismatch`. Nothing is written that claims a backup
  lies in the Drive. Until now the answer was thrown away and a mangled upload passed as a success —
  it would have come out at the restore, the one moment that must not fail;
* the answer named no checksum → one `files.get?fields=…,md5Checksum,sha256Checksum` is sent after
  it (5 quota units, and the only documented way to ask). If Google answers and still names none,
  the upload counts, `drive_md5` stays `NULL`, and the row is shown as **unconfirmed**
  (`Backup.drive_verified: false`). Google promises none of it: for the answer that closes a
  resumable session the guide names no fields at all — it comes "along with any metadata associated
  with the resource" (developers.google.com/workspace/drive/api/guides/manage-uploads) — and in the
  reference the MD5 "is only applicable to files with binary content in Google Drive" while the
  SHA-256 is there "if available". No check is no check, and calling it sound would be the same
  fault one floor higher.

**An unanswered `files.get` is not the same as "no checksum offered", and is a failed run.** If that
call fails — a 404 that denies the file, a 5xx, a Google that cannot be reached — the panel knows
nothing at all about what is up there, not even that it exists. It ends the run with
`drive_unconfirmed` and **writes no file id down**; the hourly sweep takes the orphan out of the
Drive, because an archive no backup points at is removed. The state the owner sees is a failed
backup, which is true, instead of a green one whose file Google has just denied having
(`a_file_google_denies_having_afterwards_is_no_backup_at_all`,
`an_upload_google_will_not_speak_about_afterwards_is_no_backup`).

The size is compared as well because it is all that is left when the checksum is missing, and
because Google documents **no way to send a checksum with an upload** — the check can only be made
afterwards. `drive_md5` is deliberately **not** a value of `drive_state`: the sweep rewrites that
column every hour, while a confirmation is a fact about one moment of one upload. What is written
down here is read back in two places — the hourly sweep (section 6) and the restore (section 5) —
because a checksum nobody ever looks at again is a note, not a guard.

Measured: `an_archive_that_arrives_mangled_is_no_backup_at_all` — the double flips a byte of every
chunk as it arrives, and the run fails, the Drive keeps nothing but the folder, and the row holds no
file id. Beside it `a_backup_google_names_no_checksum_for_is_kept_and_called_unconfirmed` and
`a_silent_upload_answer_is_followed_by_asking_google_outright`. The multi-chunk test above now also
holds the stored md5 against Google's, which is the guard against a digest that counts a resent
chunk twice.

---

## 4b. When the panel restarts in the middle of an upload

Google keeps a resumable session open for a week ("A resumable session URI expires after one week"),
and this panel is built to be restarted while it runs — the supervisors survive it, so a restart is
an ordinary event. Until `0018_drive_upload_sessions.sql` it still threw away half an upload: the
session address lived in a local variable of the sending task and nowhere else. On a two-gigabyte
world over a domestic line that is the difference between a backup and no backup.

**What is kept, and where.** The row in `drive_uploads` holds the backup, the user, the size the
session was opened for, the modification time and inode of the archive it was opened for, and the
moment Google handed the address out. **The address itself is not in the database.** Whoever has it
can write into that user's Drive; it is kept exactly as 0012 keeps the refresh token, and
`Keys::forget_user` therefore takes every open session with it when an account is let go. The row is
the authority over the pair: a row whose file is missing starts the upload over, a file whose row is
missing is wiped at the next start. Both are safe, so neither is a failure.

**The offset is deliberately not kept.** Google's own instruction is to ask — an empty
`PUT` with `Content-Range: bytes */<total>` answers `308` with a `Range` header, and "don't assume
that the server received all bytes sent in the previous request". A remembered offset would be a
second opinion about a fact only Google holds. `Account::carry_on` therefore always asks first:

* `200`/`201` — the upload was finished before the restart and only the answer was lost. Nothing is
  sent; the local archive is read once, has to match the mark below, and its checksum is then held
  against Google's, the same as after a normal upload
  (`an_upload_that_finished_before_the_restart_is_not_sent_a_second_time`).
* `308` — carry on at `Range`-end plus one, not at any number of ours
  (`an_upload_carries_on_where_the_restart_left_it`).
* `404` — the session is gone: throw the row and the address away and open a new one, in the same
  run. That is documented; `410` is **not** documented for Drive and nobody should write that it is
  (`a_session_google_has_forgotten_starts_again_instead_of_failing`).

**The dangerous case, and the only defence that holds.** Pressing Retry on a failed backup packs a
new archive under the *same* backup id to the *same* path. Resuming an old session into a new
archive would put the first half of Monday's world and the second half of Tuesday's into one file
that Google then reports as a complete, healthy backup. Drive documents no defence: no `Content-MD5`
header on upload, no per-chunk check, no way to cancel a session. So the client has to refuse.

Size, modification time to the nanosecond and inode are compared first
(`half_of_one_archive_is_never_glued_to_half_of_another`), but all three are metadata, and metadata
is a hint: `touch -d` puts the nanosecond back, a repack under the same name keeps the inode, and
two packs of one world are easily the same length. A siege did exactly that and landed 16 MB in a
Drive that were neither Monday's archive nor Tuesday's, with no error anywhere, because Google
named no checksum for the spliced file and there was nothing else left to catch it.

**So the bytes are the evidence.** `0020_drive_upload_prefix.sql` adds two columns to
`drive_uploads`: `offered_bytes`, how far this session has been fed, and `offered_sha256`, the
SHA-256 of the local archive up to exactly that mark. **The mark is written before the chunk goes
out**, never after it, so Google can never hold more than the mark covers
(`how_far_an_upload_has_come_is_written_down_before_the_chunk_goes_out`). A resume asks Google how
much it holds, reads the local archive back to the mark and compares. What Google holds lies inside
what was proved, so no window is left between the two:

* the digest is the one that was written down → carry on at `Range`-end plus one;
* the digest differs, or there is no mark (a session from before this migration), or Google claims
  more than the mark covers → the session is let go and the archive goes up again from the front.
  That costs one upload; a chimera costs the restore
  (`a_swapped_archive_is_caught_even_when_google_names_no_checksum_at_all`,
  `a_session_that_carries_no_mark_is_begun_again_rather_than_carried_on`,
  `google_holding_more_than_the_mark_covers_is_begun_again_from_the_front`).

Reading the archive back costs nothing extra: the sender has always had to read the confirmed prefix
to carry its running digest forward, and `upload::prefix_of` now takes the proof out of that same
pass. SHA-256 and not MD5 here, although the comparison with Google is MD5 whenever that is all
Google names — this digest is never sent anywhere, it is the panel holding one of its own files
against another, and an owner who wants a chimera in their own backup is exactly the person who
could hand an MD5 two archives that agree. The checksum of section 4 stays where it was: the second
net, not the first.

**The archive can also be rewritten while the run is still going.** The print — size,
modification time to the nanosecond, inode — is taken before the first chunk and held against the
file once more before the upload is confirmed. Has it moved, the file in the Drive is deleted and
the run ends as `drive_checksum_mismatch`: what lies there is the front of one archive and the back
of another, and no mark can catch it, because both halves came out of a single run. That is one
`stat` against a restore that unpacks into rubble
(`an_archive_rewritten_under_a_running_upload_never_becomes_one_backup`). What it does not catch is
a writer that puts length, inode and modification time back afterwards. Catching that would mean
reading the whole archive a second time after it went up — more expensive than the attack is worth,
since the backups directory is `0700` and the only writer inside it is the panel itself.

**When a session is worth nothing.** The archive has been deleted (the session refers to bytes that
no longer exist), the address file is gone, the archive has a different length, or Google's week has
run out. The week is counted from the moment the address was handed out, which is the safe reading
of a documentation that says both "expires after one week" and "expire after one week of inactivity"
and never resolves the two; our clock only prunes, and a session is dead when Google says `404`, not
when we say so. `SESSION_LIFE` is six days for that margin.

**A 308 with a `Location` is the protocol talking, not a redirect.** `reqwest` follows `308` by
default and would re-send the whole chunk to the new address behind our back, so our own handling of
a moved session never ran. The client that speaks to a session URI therefore has redirects switched
off, and the moved address is written back to disk
(`an_address_google_moves_is_the_one_kept_for_the_next_try`). What is not covered: a session Google
moves and a restart that lands before the next graceful pause. Then the old address answers `404`
and the upload starts over — the old behaviour, not a corrupted one.

**Who resumes, and when.** Nobody automatically. A start that quietly begins pushing gigabytes is a
surprise, and a panel is often restarted precisely when the machine is under strain — the games come
first. What the panel does at start is keep, not send: `Drive::pick_up` lets go of the sessions past
Google's week and wipes addresses that belong to no row, and nothing else. The resume is the Retry
button that `Backups.vue` already has: `Backups::retry` no longer deletes an archive that has a live,
matching session, and `run_create` sends it instead of packing it again
(`a_retry_after_a_restart_carries_the_upload_on_instead_of_packing_again`). Two runs cannot hold one
session: the claim is a set in the process, because a panel is one process on one machine and a claim
that outlived the process would be a lie (`two_runs_never_hold_the_same_session_at_once`).

---

## 4c. The token that outlives no upload, and two ceilings that are not a bad moment

**An access token lives an hour and a 2 GB archive on a domestic line does not.** Fetching one
before the loop and never looking at it again means a long upload is *guaranteed* to run past the
end of its own credentials. So the sender does not hold a token at all: it holds a `Bearer`
(`drive/oauth.rs`), asks it for one **before every chunk**, and `Access::usable` keeps a minute in
hand — a token that would die halfway through the next 8 MiB is replaced before that chunk goes
out (`a_token_that_dies_mid_upload_is_renewed_before_the_next_chunk_and_not_after_a_401`).

**A `401` that still arrives is one stale token, not a withdrawn connection.** It is renewed once
and the same chunk goes again. The only thing that means "the owner took the access back" is a
refresh that Google answers with `invalid_grant`, and that one belongs in the **status line of the
account** — `mark_revoked` writes state `revoked` and a sentence — not merely in the error of a run
nobody is watching. A `401` that survives a token minted a moment earlier is therefore reported as
a plain `401` refusal and **not** as `drive_revoked`: telling somebody to reconnect a connection
that still works sends them somewhere there is nothing to do
(`a_chunk_that_comes_back_401_is_a_stale_token_and_not_a_withdrawn_connection`,
`a_401_that_survives_a_fresh_token_is_not_dressed_up_as_a_withdrawal`).

**Two uploads of one account mint one token between them.** Without that, each would fetch its own
and the later one would quietly leave the earlier holding a token it had just replaced.
`Account::renewing` is the one door, checked on both sides of it, and `Access::newer_than` is how
the second one recognises that somebody else has already been through: a mint number, not the token
text, because two tokens can read the same
(`two_uploads_at_once_do_not_pull_the_token_out_from_under_each_other`).

**Before a session is opened, `about.get` is asked whether the archive fits**
(`storageQuota(limit,usage)`, developers.google.com/workspace/drive/api/reference/rest/v3/about).
Starting an upload that cannot finish costs the whole archive's bandwidth to learn what one call
answers at the door. **A missing `limit` is a Workspace account with no limit — a case, not an
error** — and nothing is refused on the strength of it
(`a_drive_with_less_room_than_the_archive_is_told_so_before_a_byte_leaves`,
`a_drive_that_names_no_limit_is_a_case_and_not_a_refusal`).

**The other ceiling is 750 GB a day, and Google hands it to us dressed as a bad moment.** Users
"can only upload 750 GB per day between My Drive and all shared drives", and whoever reaches it
"can't upload or copy additional files until 24 hours have passed". The documented refusals for
going over are a `403` "User rate limit exceeded" and a `429`
(developers.google.com/workspace/drive/api/guides/limits); *which* `reason` string arrives for the
daily ceiling in particular is **not documented**. Both of those are exactly what
`is_worth_repeating` calls worth waiting out, and no amount of waiting inside one run gets past a
ceiling that lasts a day. The defence therefore has to be a count of our own: `drive_daily_uploads`,
one row per account per UTC day, bytes Google acknowledged. A run that finds the day spent never
opens a session, and a run that spends it stops where it is and keeps its session for tomorrow
(`an_account_that_has_spent_its_day_at_google_never_opens_a_session`).

**Which 750 GB, Google does not say.** Every page writes `GB` and none of them says whether that
is 1000³ or 1024³ bytes, so `day::CEILING` takes the **decimal** reading, 750 × 1000³ =
750,000,000,000: it is the smaller of the two, and only the smaller one keeps the promise this
count is built on under both readings — the binary reading would count 55,306,368,000 bytes (7.4 %)
past Google's own limit if the decimal one is what Google means, and the panel would then stop
*later* than Google instead of earlier
(`day.rs`, `the_ceiling_reads_googles_750_gb_as_the_smaller_of_the_two_prefixes`).

**And the figure is shown in the unit it was published in.** The panel-wide `useFormatBytes` counts
in 1024s, so it would print the ceiling as "698.49 GiB" — a number nobody can hold against Google's
page. The two day figures therefore go through `useFormatDecimalBytes`
(`web/src/composables/format-bytes.ts`), which counts in 1000s: the limit reads "750 GB", word for
word what Google writes, and what has been sent reads in the same unit, so "x of y" is a comparison
and not a currency conversion. Drive **storage** keeps the binary formatter, because there the
opposite is true: `about.get` answers 16,106,127,360 bytes for the free tier and Google's own help
calls that "up to 15 GB", so 1024s reproduce the label the same user sees inside Drive.

**It is a state, not only an error.** `uploaded_today_bytes` and `daily_upload_limit_bytes` are in
every `DriveStatus` and every admin line, so the account page shows "x of 750 GB" with a bar all the
time and says so plainly once it is spent — instead of a backup that simply stops with a sentence
about rate limits. What the count cannot see: **the 750 GB belong to the Google account, not to this
panel**. Whoever uploads to the same Drive from his phone spends it too, and that case still arrives
as the `403` it always did.

---

## 5. Restoring from the Drive

Download, then the existing `unroll` — **no second restore path** (`backups/mod.rs`, `bring_down`
before `unroll`).

1. `files.get?fields=id,name,size,trashed,md5Checksum,sha256Checksum,isAppAuthorized`. Both
   checksums, because neither is deprecated and neither is certain — the MD5 "is only applicable
   to files with binary content in Google Drive", the SHA-256 is there "if available" — and
   `isAppAuthorized` because it is Google's own answer to "was this file ever opened by this app"
   (`Whether the file was created or opened by the requesting app`) — a `false` there
   is the announcement of the `appNotAuthorizedToFile` that the download will come back with, and
   it goes into the log before the bytes are asked for
   (developers.google.com/workspace/drive/api/reference/rest/v3/files).
2. **Check for room before anything runs** (`room_for`): the archive plus the estimated unpacked
   size, plus `disk::guard` for the owner's pot. *A finding on the side, so that it does not slip
   through as a silent rebuild:* `unroll` checked **none** of that: with a local backup the archive
   was at least already there, on the Drive path it has yet to arrive.
3. `GET …/files/<id>?alt=media` into a `.part` file (progress 0 → 0.4), with
   `Range: bytes=<what is already here>-` when there is something to carry on from.
4. **The checksum over the whole result** — `md5Checksum` if Google names one, otherwise
   `sha256Checksum`. Half a download is half a server; both digests grow while the bytes are
   written, so without a second read of what arrives (`files.rs`, `download`).
5. **The checksum that was written down when it went up**, held against the same digest — see
   below.
6. Rename, `unroll` unchanged (0.4 → 1.0), delete the local copy at the end.

**Google's checksum proves the transfer; ours proves it is still our archive.** Point 4 holds what
came down against what Google says about that file *this minute*, and that is an unbroken transfer
and nothing more. The file lies in the **owner's** Drive: `drive.file` narrows what the panel may
see, not what the owner may do, and any other tool of his can write over that file. Google would
then name the new checksum, the download would agree with it, and the panel would unpack a
stranger's archive over a world with nothing lit up anywhere. Until now `backups.drive_md5` was
written by the upload (0017) and never read again. So `bring_down` hands it down into the fetch as
well, and the digest of what arrived is held against it too — **in addition to** point 4, not
instead of it. Do they differ, the run ends as **`drive_file_replaced`** and not as
`drive_checksum_mismatch`: nothing is damaged and nothing broke on the wire, the file is simply no
longer the one the panel put there, and the sentence says that in those words instead of leaving
the owner to guess. The `.part` is thrown away, because there is nothing to carry on into, and
`worth_carrying_on` names the case (`backups/mod.rs`). Where the transfer check of point 4 vouched
for the bytes, the row is marked on the spot (`store::note_content_changed`) rather than left for
the next hourly round, so the page turns red in the same minute and the next press of Restore is
refused before a byte moves. Where Google named nothing to vouch with, nothing is written down: an
uncertain reading must not leave a permanent mark.

**Like is compared with like, whatever Google names today.** What was written down is an md5 of
*our* bytes, and it is held against the md5 of the bytes that arrive — `files::download` grows both
digests while it writes, so nothing is read a second time. Which algorithm Google offers this
minute therefore changes nothing at all: it may have named a sha256 when the archive went up and an
md5 today, or the other way round. Google's name is used only for the transfer check of point 4,
which takes whichever of the two it gives (`Fetched::holds`). And where nothing was written down —
`drive_md5 IS NULL`, the unconfirmed state of 0017 — there is nothing to hold anything against; that
goes into the log as its own case and is never passed off as a comparison that came out well. When
Google names no checksum at all this minute either, the sentence says so as well, because then a
broken line and a swapped file look the same from here and a second attempt is what tells them
apart.

Measured: `an_archive_swapped_in_the_drive_for_one_of_the_same_length_never_comes_back_as_a_restore`
puts a second archive of exactly the same length under the same file id — Google names its new
checksum cleanly, the download matches it, the size matches the one that was recorded — and the same
fetch is run twice: once with only the size written down, where the stranger lands whole on the disk
the way it always did, and once with the checksum, where it is refused. The counter-test
`an_untouched_archive_comes_back_even_when_google_names_another_kind_of_checksum` has Google name
only a sha256 and breaks the download in half on the way, and the untouched archive still comes back.
At the button a person presses: `an_archive_that_is_no_longer_ours_is_never_unrolled_over_a_world`.

The round trip is measured: `a_backup_comes_back_out_of_the_drive_again` packs a world up, destroys
it, fetches it back and compares the contents — and checks that nothing local is left afterwards and
that the archive is still in the Drive.

### 5a. The way back carries on too

**A backup you cannot fetch back is not a backup.** The upload has been resumable since 0018; the
way back began at zero on every attempt, and because the checksum guard deletes a half archive
there was not even a stump to start from. Measured before it was fixed: an abort halfway, the guard
deletes, the next attempt fetches the whole file again, and **not one `Range` header ever left the
machine**.

Google documents the way (guides/manage-downloads): "Partial download involves downloading only a
specified portion of a file. You can specify the portion of the file you want to download by using
a byte range with the `Range` header. For example: `Range: bytes=500-999`". So:

* The `.part` file **stays** when the line dies. `bring_down` deletes it only on a cancel, on a
  file Google no longer has, on the abuse refusal, and when the checksum came out wrong — the
  cases where the bytes on disk are worth nothing. Everything else is worth carrying on
  (`backups/mod.rs`, `worth_carrying_on`).
* The next attempt asks for `bytes=<the length that is here>-`, seeds both digests with the bytes
  already on disk and appends. **The checksum is over the whole file**, never over the last
  piece — otherwise the way back would grow the same chimera the upload was caught with.
* **Nothing is resumed that cannot be checked afterwards.** If `files.get` names neither checksum,
  the half is thrown away and the file comes down from the front.
* A `.part` carries a note beside it, `<archive>.part.source`, holding `<file id> <size>
  <checksum>` as Google named them when the download began. Does the note not match what Google
  says this minute, the half belongs to another file and is thrown away rather than glued on
  (`half_a_download_of_another_file_is_never_glued_to_this_one`). The note is deleted with the
  half, and `forget()` takes both when a backup goes.
* A `200` where a `206` was asked for is a server that ignored the range: the file is truncated and
  written from the front. A stream that ends clean but short ends the run with "Google broke off
  after n of m bytes" and keeps what came.
* **The length the panel wrote down when the archive went up is the third opinion.** Google
  documents `size` as "Size in bytes of blobs and Google Workspace editor files. Won't be
  populated for files that have no size, like shortcuts and folders"
  (reference/rest/v3/files), so an archive of ours always has one — but the field is optional in
  the answer and the code has always read it as optional. A siege took that seriously: no `size`,
  no checksum, and a stream that ended clean at two fifths of the file. Nothing held it to a
  length, `fetch` called it a finished restore, and the truncated archive went into `unroll`.
  `bring_down` now hands `backups.size_bytes` — the length this panel itself uploaded — down into
  the fetch. A Drive that names a different length for that id is refused **before** a byte is
  asked for, because that file is not the archive that left this machine, and an arrival short of
  the recorded length ends the run instead of the restore
  (`an_archive_google_names_neither_a_size_nor_a_checksum_for_is_not_called_whole`,
  `a_file_google_names_another_size_for_than_the_one_that_went_up_is_never_fetched`).

Measured: `a_download_that_breaks_off_halfway_carries_on_where_it_stopped` (the exact `Range` that
goes out is asserted) and, through the whole restore, at the button that a person actually presses,
`a_restore_that_breaks_off_halfway_carries_on_at_the_next_press`.

### 5b. The file Google calls abusive

"Files identified as abusive (such as harmful software) are only downloadable by the file owner.
Additionally, the `acknowledgeAbuse` query parameter must set to `true` to indicate that the user
has acknowledged the risk of downloading potentially unwanted software or other abusive files."
And, in the same breath: "Your application should interactively warn the user before using this
query parameter" (developers.google.com/workspace/drive/api/guides/manage-downloads).

A backup archive is a whole server tree — jars, mods, plugins. This is not a corner case, and it
used to come out of the panel as a bare `drive_unavailable` with no way forward at all.

**A silent `acknowledgeAbuse=true` is out of the question.** It would turn the panel into a
delivery service for whatever Google's scanner just found, with nobody asked. So the parameter is
set in exactly one place and only when it was handed in from outside (`files.rs`, `Fetch`), and
the way in is:

1. Google refuses with `403`. The classifier turns it into `DriveError::Abusive` and the operation
   error `drive_abuse_blocked`, which is never repeated — no backoff opens this door. The `reason`
   string Google sends (`cannotDownloadAbusiveFile`) is on **none** of the pages under
   guides/handle-errors, so it is matched but nothing else is bent to look like it: another 403
   stays the plain refusal it is, with Google's own sentence.
2. `web/src/pages/servers/Backups.vue` reads that code off the failed run (`abuseBlocked()`) and
   turns the failure notice into the warning: Google calls this archive malware or spam, and if
   you did not pack a suspicious mod, restore an older backup instead. The only control it offers
   is **"I accept the risk, fetch it anyway"**, right under the sentence — a phone has no tooltips
   (22.10), so the warning is text, not a hover.
3. That press sends `POST …/backups/:backup_id/retry?acknowledge_abuse=true` (10.7). The panel
   holds the acknowledgement for **one run**, in memory, keyed by the operation
   (`backups/mod.rs`, `warned`), and a run that fails again needs a fresh press. A warning that is
   given once and remembered for ever is no longer a warning — and an acknowledgement that
   outlived the process would be a lie, the same reasoning as the upload claim in 4b.

Measured in both places: `a_file_google_calls_abusive_is_only_fetched_after_a_person_has_said_yes`
counts that **zero** acknowledgements went out unasked, and
`an_archive_google_calls_abusive_comes_back_only_after_the_owner_says_yes` walks the whole way from
the failed restore over a plain retry (still refused, still nothing acknowledged) to the press that
says yes. `web/src/pages/servers/backup-abuse-path.test.ts` watches the chain from the button to
the query from outside, and counts the places that may write `acknowledgeAbuse`: exactly one.

---

## 6. The sweep

**One per connected user, once an hour**, spread out like playit's reconcile (`offset_of`, with the
test that 64 users do not land on the same second and that a restart does not reshuffle the order).
Three calls: refresh the token (**that** is the check for `invalid_grant`), `about.get` for the
storage state, and **one** `files.list` for all backups of that user.

| Situation | `drive_state` | Consequence |
|---|---|---|
| file there | `present` | nothing |
| in the trash | `trashed` | the list shows it; restoring refused |
| file missing | `missing` | the list shows it; `409 backup_not_restorable`; deleting stays allowed |
| disconnected with `keep` | `unreachable` | the row stays; connecting again finds it |
| orphan | — | gets deleted, with an `info!` line |

`the_sweep_notices_a_file_that_was_deleted_or_binned` covers the first three.

**The orphan rule has to stay narrow**: one that is too wide deletes something in a person's storage
that belongs to them. It only applies if a file carries *our* stamp, names a `backup_id` and no row
of that user holds exactly this `drive_file_id`; the folder itself is exempt.
`the_sweep_takes_an_orphan_and_never_a_strangers_file` puts both into the same folder and checks both
directions.

**The sweep reads the checksum too, and it costs no extra call.** `files.list` already asks for
`md5Checksum` in the same `fields` it asks `trashed` in, so the answer carries it whether it is
looked at or not. Where a row has an md5 written down and Google names one for that file, the two
are compared, and a difference sets `backups.drive_content_changed_at` (0021) with a `warn!` line.
That is the whole point of doing it here rather than at the restore: a backup that is no longer the
backup is something the owner has to learn while he still has a night to make another one, not in
the minute he needs this one. It clears itself as well — the next round that finds the two equal
again writes `NULL` back, so an owner who puts his own file back is not left carrying a red mark.
Nothing is compared where nothing can be: a row without `drive_md5` (unconfirmed) and a file Google
names no md5 for this round are both left exactly as they were, because silence is not evidence.
The one case the sweep cannot judge is a file Google describes only by its sha256 — what we wrote
down is an md5, and the panel will not fetch a whole archive down to find out. That one keeps its
green badge until the restore reads the bytes and settles it. Measured:
`the_hourly_look_finds_a_swapped_archive_before_anybody_needs_it`, which also holds the sweep to
zero `files.get` calls.

| Situation | Column | The page |
|---|---|---|
| checksum equal | `drive_content_changed_at` `NULL` | nothing |
| checksum differs | the moment it was first seen | red badge "Not this backup any more"; restoring refused |
| nothing to compare | stays as it was | unchanged |

`files.list` pages (`pageToken`), and that is not incidental: taking a first page for the whole truth
would mean the sweep considers every file behind it an orphan.

---

## 7. Local stays possible

**Yes, local stays.** Three reasons, and the first is compelling:

1. Today there is neither a Google project nor a client secret. A panel that cannot back up without
   Drive **cannot back up today**.
2. Free Google storage is 15 GB, shared with Gmail and Photos. According to reports new
   accounts have only had 5 GB since March 2026 (15 only with a phone number on file;
   9to5google.com, 2026-05-14, and Google has written "**up to** 15 GB" in its help since then). A
   modpack world is 2–5 GB.
3. Existing backups must not disappear. `backups.location` has `DEFAULT 'local'`, so every row from
   0002 is local, and nothing was touched. Measured:
   `switching_the_target_leaves_the_backups_that_exist_alone`: a local backup stays local after
   switching, its archive stays put, it downloads and restores.

The choice is the operator's: `drive_settings.target_policy ∈ { user_choice, drive_only, local_only }`,
default **`user_choice`**.

**`drive_only` has no local fallback, and that is the point where a bug arose during the build and a
test caught it.** At first `target_of` returned `local` for `drive_only` without a connected Drive:
the bytes would have landed on exactly the disk the operator ruled out, and nobody would
have noticed. Now the target stays `drive`, and the run is refused
(`drive_only_refuses_a_backup_rather_than_falling_back_to_our_disk`). With `user_choice` the fallback
is right, on the other hand: a backup on the wrong disk is better than none, and `reason` tells the
interface why the switch shows `local`.

---

## 8. Quota, disk limit, schedule

* **The count quota (10.12) counts Drive backups too.** If they did not count, "50 local and 50 in
  the Drive" would be a way around 10.12.
* **The disk limit (12.7) does not count them.** That is the **one line** where a bug would have
  arisen: `auth/disk.rs` sums `backups.size_bytes` over all servers of the account, and without
  `AND b.location = 'local'` a backup of which not one byte lies here would charge the pot forever.
  `size_bytes` stays set (10.1 shows the size), only the sum leaves Drive rows out. Measured through
  the real meter and the real door (`disk::guard`):
  `a_backup_in_a_drive_does_not_hold_the_owners_disk_quota`. Counter-check run: line removed →
  `disk_limit_reached` at 1000 of 1024 MiB, test fails; turned back, green again.
* **On the way it does count**, and that is right: the archive really is on the disk. The error
  message of `507 no_space` now says so word for word ("a backup into Google Drive is built here
  first as well"). Limit of the honesty: `Disks` remembers for 60 seconds (`auth/disk.rs`,
  `WINDOW`), so a file just deleted still counts for up to a minute. That is already the case today.
* **Schedules carry unchanged.** Cleaning up still happens only among automatic backups; for a Drive
  row, cleaning up means `files.delete` and then the row — row first, file second, like `forget()`.
  If `files.delete` fails, the file stays in the user's Drive, with a `warn!` line: it lies in
  *their* storage, they see it and can throw it away.
* **The safety copy of a restore follows the server's target**, not the location of the restored
  backup: otherwise the path that fetches a Drive backup back would be exactly the path that fills
  the local disk (`the_safety_copy_of_a_restore_follows_the_servers_target`).

---

## 9. Downloading: a link, not a pass-through

`Backup.drive_web_link` is `https://drive.google.com/file/d/<id>/view`. The file belongs to the user,
they are signed in at Google, and **the panel transfers not one byte.** 10.8 and 10.11 answer
`409 backup_lives_in_drive` with the link in the `message`
(`a_drive_backup_is_not_downloaded_through_the_panel`).

Passing it through would cost exactly the bandwidth this area is supposed to save, **twice**. It is
not built. On the side, the link route sidesteps the known limitation of 10.11 that
`BackupItem.vue:116` writes `https://` fixed into its URL.

---

## 10. The wiring, to be ticked off

The most frequent mistake of this project is "built, but not connected". So here is the list, and
every line has been checked:

1. `main.rs`: `mod drive;` · `Drive::new(...)` + `drive.start()` · `Arc::clone(&drive)` to
   `Backups::new` · `.merge(api::drive::router(...))` in the API router.
2. `api/mod.rs`: `pub mod drive;`.
3. `api/backups.rs`: `…/backups/target` with `get`/`put`, before the parameter route (1.3).
4. `web/src/pages/admin/routes.ts`: `admin-drive`, route **and** menu entry in one.
5. `web/src/pages/account/sections.ts`: `{ id: 'drive', component: AccountDrive }`.
6. `web/src/pages/servers/Backups.vue`: target display, switch, warning, "Open in Drive".
7. `web/src/api/types.ts` and `model.rs`: the same three fields on `Backup`.
8. Resuming an interrupted upload gets **no control of its own**: it hangs on the Retry button that
   `Backups.vue` already shows for a failed backup, so there is nothing that could be built and then
   left unreachable. Carrying on an interrupted **download** hangs on the same button.
9. The one control this area did have to add is the abuse acknowledgement (5b): the warning and the
   button "I accept the risk, fetch it anyway" stand in the failure notice of
   `web/src/pages/servers/Backups.vue`, in the place where a person triggers a restore, and
   `backup-abuse-path.test.ts` walks that chain from outside.
10. The day's share of Google (4c) is a **figure, not a button**: `uploaded_today_bytes` and
    `daily_upload_limit_bytes` ride along in `DriveStatus` and in every admin line, the account page
    draws them as a third tile with a bar next to the storage tile, and the admin overview shows
    them under each account. Nothing here needs pressing — the only thing a person can do about a
    spent day is wait, so the panel says so instead of offering a control that would do nothing.

---

## 11. What explicitly is not built

* **No encryption of the archives.** A backup contains the whole server tree, so also
  `server.properties` with the RCON password and plugin credentials; unencrypted, Google can read
  every backup. Encrypted, the file in the Drive would no longer be usable with one click, and a
  forgotten passphrase is a lost server. **The user is told**: the account page says it in one
  sentence before they connect.
* **No deleting in a stranger's Drive by an admin.** `DELETE /admin/drive/:user_id` disconnects and
  leaves everything in place; there is no `?files=` with which they could do otherwise
  (`an_admin_disconnecting_somebody_leaves_every_file_alone`).
* **No pass-through**, **no `drive.appdata`**, **no domain management**, **no second provider**, **no
  subfolder per server** (one folder per user, one call for the sweep; the server name is carried by
  the file name).
* **Cost: none.** Google Cloud project, Drive API and device flow are free at this scale. Money is
  only paid if a *user* buys more storage — their money, not the operator's, and exactly the point
  of all this.

---

## 12. What stays open

1. **No test against the real Google.** There is no project to run one with, and a test that writes
   files into a real Drive does not belong in a test suite. Everything here is measured against
   `drive/harness.rs`. The first real connection is therefore the first contact with Google's actual
   responses; the places where that could hurt are the field names and the error shapes, and those
   are tested one by one (`http.rs`, `oauth.rs`).
2. **`external_services_enabled` and the revocation.** With the switch from 12.10, disconnecting
   sends no revocation to Google (it would be a call outward). The token then still sits in
   *Google's* list until the user removes it there themselves. The file here is gone. Getting out of
   the corner mattered more.
3. ~~**A deleted panel account does not revoke.**~~ Closed: `api/admin.rs`, `dispose_of` calls
   `Drive::dispose_of` when a user is deleted (12.6), so the token is handed back, the key files go
   and the row disappears through `ON DELETE CASCADE`.
4. **No mail on revocation.** The hook sits in `mark_revoked` and writes `warn!`. As soon as 19 is
   there, the transition `connected → revoked` is the natural trigger, and it would be a ninth
   template.
