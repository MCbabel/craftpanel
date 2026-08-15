# Google Drive

Status: 2026-08-14. Design and build record for section **22** of the contract: backups into the
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
issued a refresh token expiring in **7 days**"
(developers.google.com/identity/protocols/oauth2). On "Testing" every connection therefore breaks
after a week — silently, and exactly when somebody needs a backup.

**The panel cannot query this state**; Google offers no interface for it. It can only say it and
guess: if a refresh fails with `invalid_grant` on a token younger than ten days, `last_error` says
word for word that this looks like a consent screen on *Testing* (`oauth.rs`,
`looks_like_a_testing_project` and `TESTING_HINT`; the test for it is
`a_withdrawn_connection_is_written_down_and_the_key_file_stays`).

The age of the connection has **no column**: it is the mtime of the token file (`keys.rs`,
`token_written_at`). The same measurement without a migration — and a token overwritten by a fresh
connection is thereby rightly young again.

Publishing needs **no** review process, because `drive.file` is listed under "non-sensitive" in
Google's table (developers.google.com/workspace/drive/api/guides/api-specific-auth) and an app with
non-sensitive scopes only does not go through the review
(developers.google.com/workspace/guides/configure-oauth-consent).

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
| access token | **in memory only**, `RwLock<Option<Access>>` with a deadline | — |
| `device_code` of a running operation | **only in the polling loop** | — |
| client id | database, `drive_settings.client_id`, not a secret | — |

The `Secret` newtype (`oauth.rs`) gives only `Secret(hidden)` as `Debug`; `expose()` is called in
exactly three places: token request, revocation, key file.

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
words, access token in memory gone, **and the key file stays.** The model:
`PlayitStatus.configured` means "A key is here. Says nothing about whether it still works."
(`playit/mod.rs:103-104`). The state belongs in the column, not in the presence or absence of a
file.

The hook for the mail that 22.17 names as the fourth net sits in `Account::mark_revoked` and today
writes a `warn!` line.

**`invalid_grant` does not mean the same thing everywhere.** The parser turns it into `Revoked`
everywhere, but on a **poll** of the device operation it means no withdrawn connection at all, only
that this code is used up (22.4), and the whole answer to that is a new code. So the polling loop has
to tell the case apart itself (`drive/oauth.rs:305-307`).

**And `revoked` is the only state that holds up a backup run.** Because the key file stays, the mere
presence of a file kept saying "connected" while every upload failed, so the door in front of a run
asks for the file **and** the column. `error` explicitly does **not** belong there: a sweep that
failed to reach Google once is no reason to refuse a backup that could succeed
(`drive/mod.rs:491-499`).

**The panel-wide Drive settings live in a row of their own, not in `panel_settings`.** That row is
written as a whole by 12.11; a second area writing into it would put a second hand on
`auth/settings.rs`, `api/admin.rs` and the settings page (`drive/store.rs:16-18`).

---

## 4. The flow of a backup: build → upload → delete locally

**The first reason is compelling and comes from what exists, not from taste.**
`quiesce::Held::take` switches `save-off`, `Drop` switches it back, and `quiesce.rs:5-8` names "the
whole risk of this area: a server left with saving switched off looks perfectly healthy and loses
everything since the last flush the moment it crashes." **A streaming upload would hold `save-off`
for the entire upload**: at 2 GB and 10 Mbit/s that is half an hour without saving.

That is why the upload sits **outside** the bracket: `pack()` holds it and gives it back,
`deliver()` uploads afterwards (`backups/mod.rs`, `run_create`). The comment with the reason stands
at the spot, and a test nails it down:
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
* `404` means "session expired".
* `5xx`/`429`: `min(2^n + jitter, 64 s)`, five attempts **per chunk**, not per upload: a run over
  250 chunks that gave up somewhere after five bad moments would never get through on a bad line.
* **The loop needs a counter of its own for "`308`, but nothing arrived".** Such a `308` is not an
  error, so the retry in `one_chunk` never sees it; without this counter the loop would run
  **forever** against a Google that keeps answering "I have nothing" (`drive/upload.rs:132-134`).

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
reproduces that, and the same test shows that after a failed run only the folder is there. For 5.12
(restart) there is therefore **nothing to add**.

File name: `<server>--<backup>--<created_at>.tar.zst` through the same `slug()` as a download
(`api/backups.rs`), plus `appProperties { panel, server_id, backup_id }`.

---

## 5. Restoring from the Drive

Download, then the existing `unroll` — **no second restore path** (`backups/mod.rs`, `bring_down`
before `unroll`).

1. `files.get?fields=size,md5Checksum,trashed`.
2. **Check for room before anything runs** (`room_for`): the archive plus the estimated unpacked
   size, plus `disk::guard` for the owner's pot. *A finding on the side, so that it does not slip
   through as a silent rebuild:* `unroll` checked **none** of that: with a local backup the archive
   was at least already there, on the Drive path it has yet to arrive.
3. `GET …/files/<id>?alt=media` into a `.part` file (progress 0 → 0.4).
4. **`md5Checksum` against what was computed locally.** Half a download is half a server; the
   checksum is computed while writing, so without a second read (`files.rs`, `download`).
5. Rename, `unroll` unchanged (0.4 → 1.0), delete the local copy at the end.

The round trip is measured: `a_backup_comes_back_out_of_the_drive_again` packs a world up, destroys
it, fetches it back and compares the contents — and checks that nothing local is left afterwards and
that the archive is still in the Drive.

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
  first as well"). Limit of the honesty: `Disks` remembers for 60 seconds (`auth/disk.rs:31`), so a
  file just deleted still counts for up to a minute. That is already the case today.
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
3. **A deleted panel account does not revoke.** `Drive::dispose_of` exists and does the right thing,
   but it would have to be called from `api/admin.rs` (12.6), and in this round that file belongs to
   another area. Until then, deleting a user leaves their Google permission in place; the row
   disappears through `ON DELETE CASCADE`, the token file stays.
4. **No mail on revocation.** The hook sits in `mark_revoked` and writes `warn!`. As soon as 19 is
   there, the transition `connected → revoked` is the natural trigger, and it would be a ninth
   template.
