# The overall contract

As of 2026-08-12. Merges the eight area contracts in `docs/api/` into **one** binding interface.
The eight area files stay as a derivation; where they differ from this document, this document
wins. What was decided and why is in section 6.

Source references `path:line` are relative to `vendor/modrinth/` (vendored, unmodified Modrinth
code), or to `/root/ref-modrinth/` for files that were not vendored along
(`layouts/wrapped/**`, `components/servers/ServerListing.vue` — both removed on 2026-08-12).
Checked again for this document: `layouts/` now only contains `shared/`, `components/servers/`
no longer contains a `ServerListing.vue`.

---

## 1. Basics

### 1.1 Base path and format

All endpoints under `/api/v1/`. Exactly **one** exception:
`GET /modrinth/v0/backups/:backup_id/download` (10.11) — a compatibility path, because
`components/servers/backups/BackupItem.vue:116` assembles the URL by hand and the component is
taken over unchanged.

Requests and responses are JSON (`application/json`), field names in `snake_case`. Three endpoints
differ and carry raw bytes: `PUT …/files/content`, `PUT …/operations/:op_id/payload` (both
`application/octet-stream`) and `POST …/content/upload` (`multipart/form-data`).
`GET …/files/content` and the two download endpoints answer with bytes.

### 1.2 Authentication

Session cookie `craft_session`: `HttpOnly`, `SameSite=Lax`, `Path=/`, `Secure` as soon as it is
served over HTTPS. The content is a 256-bit random number in base64url; only its SHA-256 is
stored. Lifetime 30 days, sliding, written at most once an hour. Passwords with Argon2id.
No JWT, no bearer, no token in a URL.

**What a password costs: 64 MiB, three passes, one lane** (`auth/password.rs:13-17`). That is
three times the memory OWASP asks for, and one pass more — this panel signs in a handful of
people a day and can afford a tenth of a second where a shopping cart cannot. A test build does
not carry argon2's optimizations and would need seconds per hash, so it computes with
`Params::MIN_M_COST` and one pass (`auth/password.rs:29-39`); verification reads its parameters
from the stored hash, so the tested paths are the same and only the price is different.

**Sliding means: after the handler, never before** (`auth/extract.rs:205-220`). Signing out and
changing a password withdraw exactly the session this cookie names; extending it beforehand would
put it back. And at most once an hour, because a page that asks again every five seconds would
otherwise write the same row seventeen thousand times a day (`auth/session.rs:1-6`).

The **first** admin is created by the installer through a subcommand of the binary
(`docs/PLAN.md:360`). All further accounts come about in three ways: the same CLI, an admin via
12.3, or — since section 20 — a sign-up that the operator has to unlock explicitly. Both switches
for that stand at "closed" (12.10), and without mail delivery set up (19) the door stays shut even
when it is unlocked: an account whose confirmation mail never arrives would be an account nobody
can use.

**The sender IP is in a request only if the service puts it there.** Every brake that counts by it
— the sign-in brake (3.1), the sign-up ceiling (20.11), the reset brake (21.6) — reads it from the
same extension, and `signup_ip` and `requested_ip` come from there too. That is why serving uses
`into_make_service_with_connect_info::<SocketAddr>()` and not the router alone
(`crates/craftpanel/src/main.rs:265-272`). Without that one line all three brakes **silently**
count nothing and the two columns stay empty. There is no error message that would give it away.

**Tokens outside the session.** Confirmation (20.9) and reset (21.5) each need a secret that
travels in a mail. Both are 256 random bits in base64url, both sit in the database as SHA-256
only, like the session cookie. The sentence "no token in a URL" holds unchanged for our endpoints:
the token travels in the **body** of a `POST`. It only appears in the link the human clicks in
their mail, and the page behind it clears it out of the address bar right away (20.9).

**CSRF.** `SameSite=Lax` covers the normal case. On top of that every modifying endpoint checks a
present `Origin` header against the **`Host` header of the same request** → otherwise
`403 csrf_origin_mismatch`. Against `Host` and not against a configured panel origin
(`auth/extract.rs:172-203`): that is the address the panel was just reached at, and a second,
configured one would only be one more place where something can be wrong — if it is wrong, it
locks the operator out of their own panel. `Origin: null`, which a sandboxed frame sends, belongs
to no host and is rejected. `application/x-www-form-urlencoded` and `multipart/form-data` on JSON
endpoints → `415 unsupported_media_type`. The WebSocket upgrade checks `Origin` **always**, even
when the header is missing: no same-origin rule applies to upgrades, a foreign window could
otherwise connect with our cookie.

### 1.3 IDs

All IDs are ULIDs as opaque strings, 26 characters of Crockford base32. Never sequential numbers.
The exception is foreign IDs that we only pass through: Modrinth project and version IDs
(8 characters base62), loader identifiers, build numbers and port numbers.

**Naming rule.** A resource's own identifier is called `id`, a reference to another resource
`<resource>_id` (`server_id`, `backup_id`, `user_id`, `project_id`). That holds for `Server.id`
too — Modrinth's `Archon.Servers.v0.Server.server_id` only comes about in the adapter.

**Route patterns.** Where a path segment can be a ULID and fixed segments stand next to it —
`GET …/backups/schedule` next to `GET …/backups/:backup_id` — the router has to recognize the ULID
by its pattern, not by registration order.

**Every area takes its path segments apart itself.** `axum::extract::Path` answers a malformed
ULID on its own with `400 text/plain "Invalid URL: …"`. That is neither an `error` nor a
`message` and therefore not a response the interface can read (measured on
`GET /servers/not-a-ulid/properties`). So every area has its own small extractor that answers
`404 … _not_found` in the envelope from 1.7: a segment that cannot be a ULID names nothing, and
"misspelled" and "does not exist" should get the same answer (`api/settings.rs:62`,
`api/content.rs:73`, `api/files.rs:53`, `api/console.rs:54`, `api/backups.rs:361`,
`api/playit.rs:43`, `api/mail.rs:42`, `api/registration.rs:190`, `api/admin.rs:928`,
`api/servers.rs:113`). Once every area needs it, it belongs next to `Caller` in
`crate::auth::extract`.

**Order.** The identifiers of one panel process rise strictly, even those of two rows in the same
millisecond. They come from a `ulid::Generator` behind a process-wide lock that counts on within a
millisecond instead of rolling the random part again — `ulid::Ulid::generate()` "will not
guarantee monotonic sort order" (`ulid-3.0.0/src/time.rs`) and decides the order of two
simultaneous rows by coin toss. So `ORDER BY id` is the order things came into being. A clock
turned back does not reverse it: the counter runs on from the last identifier, its time part thus
ahead of the wall clock until that has caught up again.

**What this promise does not cover.** It holds per process and per process run, because the
counter is state in memory. `craftpanel admin …` writes rows from its own process, and after a
restart the counter starts over at the wall clock; two identifiers from two processes in the same
millisecond sort in any order. Where the order of two rows from different processes matters, the
query therefore does not sort by `id` but by the database's insert counter — as the audit log in
11.9 does (`created_at`, then `rowid`).

### 1.4 Timestamps

RFC 3339 in UTC with `Z`, e.g. `"2026-08-12T14:03:11Z"`. Field names end in `_at`.

**One single exception:** `ApiFileItem.modified` and `ApiFileItem.created` are **Unix seconds as a
number**. `layouts/shared/files-tab/components/FileTableRow.vue:303,308` hardcodes
`new Date(props.modified * 1000)`; at 20,000 entries strings would be 40,000 `Date.parse` calls
for nothing. The exception holds for these two fields only.

### 1.5 Sizes

**Allocated** memory sizes carry the suffix `_mib` and are mebibytes as an integer (`memory_mib`,
`memory_max_mib`, `allocated_mib`). **Measured** sizes carry `_bytes` and are bytes (`used_bytes`,
`size_bytes`, `bytes_processed`, `ram_usage_bytes`). Reason: `-Xmx` is set in MiB and the slider
moves in MiB, while cgroup files and `stat` deliver bytes.

`ram_usage_bytes`, `ram_total_bytes`, `storage_usage_bytes`, `storage_total_bytes` in the WS
message `stats` are called that because `providers/server-context.ts:20-26` demands it.

### 1.6 Progress

Progress values are fractions **0…1**, never percent. `components/base/Admonition.vue:148` clamps
to `[0,1]`, `components/servers/admonitions/FileOperationAdmonition.vue:5` passes through. Only
`InstallingBanner` wants 0…100 and divides again itself (`InstallingBanner.vue:201`) — the
provider does that one conversion.

### 1.7 Errors

Always an HTTP status plus

```json
{ "error": "<code>", "message": "<text>" }
```

`error` is stable and machine-readable, `message` is for humans and may change. The envelope has
exactly these two fields — no `details`, no `field`, no list.

**One envelope on the wire, two types in the code, and that is not sprawl.**
`auth::error::Failure` (`auth/error.rs:1-6`) and `ops::fault::Fault` (`ops/fault.rs:8-11`) carry
the contract's stable code with them; section 5 alone owes nine that no category can spell.
`Failure` additionally carries the seconds for `Retry-After` instead of leaving them to every
endpoint: a brake that forgets the header is a client that knocks again right away.

**A third type was once planned and is not coming back.** `crate::error::ApiError` (`error.rs`)
sorted errors by category and would therefore have answered `conflict` three times over for
`username_taken`, `weak_password` and `last_admin` — the interface cannot branch on a value like
that. That is exactly what it died of: no endpoint ever called it, it stood in the compiler log
as a dead enum for months and was deleted on 2026-08-15. The envelope on the wire does not change
because of it; it never came from there.

For **every** endpoint the following hold without repetition: `401 unauthenticated`,
`403 forbidden`, `404 server_not_found` (on server-related paths), `500 internal`.
`404 server_not_found` also comes when the server exists but the caller has no `BASE_READ` on it —
otherwise the choice between 403 and 404 gives away foreign server IDs.

**And the order of the checks is part of that promise.** First the server, then the permission,
then everything about the thing itself. A `409` about an unfinished backup, answered *before* the
permission, would confirm to a stranger that the guessed ID is a real backup
(`api/backups.rs:245-246`, `backups/mod.rs:240-250`, counter-checks `api/backups.rs:584-587`,
`backups/tests.rs:977-979`). In the same way the spelling of the **second** half of a path must not
change what the caller learns about the first: `…/servers/<foreign>/members/<nonsense>` and
`…/servers/<foreign>/members/<real>` answer the same (`api/access.rs:254-256`, counter-check
`:1126-1127`).

The **eight session-free** endpoints (3.1, 20.1–20.4 and 21.1–21.3) know neither
`401 unauthenticated` nor `403 forbidden`: they check no session. `403 csrf_origin_mismatch` holds
for them too, because every modifying endpoint checks `Origin` (1.2).

Also without repetition: **every writing server-related endpoint can answer `409 server_busy`** as
long as a locking operation is running (5.8). The error lists of the individual endpoints name it
only where it has an additional, narrower meaning. The one exception is the file endpoints during
an `unarchive`: that deliberately locks nothing (5.8).

The complete code catalog:

| Status | Code | Meaning |
|---|---|---|
| 400 | `invalid_request` | body or query parameter unusable, required field missing, value out of range |
| 400 | `invalid_name` | name empty, too long, contains `/`, `.`, `..` or control characters |
| 400 | `invalid_path` | path normalization violated (N2, N3, N5 in 7.1) |
| 400 | `path_too_long` | N6 violated |
| 400 | `invalid_move` | target lies inside the source, or source is a prefix of the target |
| 400 | `not_a_regular_file` | FIFO, socket, device file |
| 400 | `not_a_directory` | listing on a file |
| 400 | `non_utf8_name` | existing entry with a name that cannot be represented |
| 400 | `weak_password` | under 10 characters |
| 400 | `invalid_email` | no address in the normal form from 20.10 |
| 400 | `invalid_reset_token` | reset token unknown, expired or used up — one code for all three (21.5) |
| 400 | `invalid_role` | server role unknown |
| 400 | `role_not_assignable` | `owner` cannot be assigned |
| 400 | `cannot_invite_self` | inviting yourself |
| 400 | `cannot_remove_owner` | the owner cannot be removed |
| 400 | `invalid_transfer_target` | target user missing, is the same one, or has no finished system user |
| 400 | `invalid_property_key` | key is not a `[A-Za-z0-9._-]+` |
| 400 | `invalid_property_value` | type, value range, line break or null byte |
| 400 | `invalid_java_version` / `invalid_jre_vendor` | not in `GET /java-runtimes` |
| 400 | `invalid_startup_command` | empty, quotes that cannot be split, line break |
| 400 | `invalid_port` | not 1024–65535 |
| 400 | `invalid_schedule` | schedule limits violated; `message` names the field |
| 400 | `memory_too_small` | under 512 MiB |
| 401 | `unauthenticated` | no cookie or an expired one |
| 401 | `invalid_credentials` | name or password wrong |
| 403 | `forbidden` | signed in, but the permission is missing |
| 403 | `forbidden_path` | resolution leaves the server root |
| 403 | `wrong_password` | old password wrong |
| 403 | `email_unverified` | password is right, the address is not confirmed yet (20.8) |
| 403 | `approval_pending` | password is right, an admin has not approved the account yet (20.8) |
| 403 | `csrf_origin_mismatch` | foreign origin |
| 403 | `cannot_delete_self` | admin deletes themselves |
| 403 | `port_out_of_pool` | port outside the pool, caller is not a panel admin |
| 404 | `server_not_found` | unknown or invisible |
| 404 | `not_found` | path in the file system does not exist |
| 404 | `parent_not_found` | parent directory does not exist |
| 404 | `user_not_found` / `member_not_found` / `invitation_not_found` | |
| 404 | `operation_not_found` / `content_not_found` / `backup_not_found` | |
| 404 | `allocation_not_found` / `loader_not_found` / `build_not_found` | |
| 404 | `log_not_found` / `log_file_missing` | log file resp. `logs/latest.log` missing |
| 404 | `runtime_not_installed` | Java runtime neither present nor obtainable |
| 404 | `invalid_token` | confirmation token unknown (20.9) |
| 404 | `registration_not_found` | no open application with this ID |
| 404 | `mail_not_found` | no row in the outbox with this ID |
| 404 | `mail_content_gone` | the row exists, its body was emptied after delivery (19.7) |
| 404 | `drive_link_not_found` | no linking operation open (22.5) |
| 409 | `server_busy` | a locking operation is running (5.6); `message` names it in plain words |
| 409 | `server_running` | action demands a stopped server |
| 409 | `server_not_running` | action demands a running server (sending a command) |
| 409 | `server_broken` | `status == "broken"`, start refused |
| 409 | `invalid_power_transition` | transition not allowed; `message` names the actual state and the wish |
| 409 | `budget_exceeded` | this action would exceed the owner's budget |
| 409 | `over_limit` | the owner is already over (an admin lowered the limit) |
| 409 | `disk_limit_reached` | the owner's disk limit is reached (12.7); nothing is deleted |
| 409 | `disk_usage_unknown` | a directory of this account stayed closed to the panel, even after the helper handed it back; how much is used cannot be said, and nothing new is written because of it (3.3, 12.7) |
| 409 | `already_exists` | target path exists and `on_conflict=fail` |
| 409 | `not_empty` | deleting a directory without `recursive` |
| 409 | `file_not_accessible` | the game process closed this folder to the panel (`EACCES`/`EPERM`); not a permission error of the caller and **not** a `500 internal` |
| 409 | `operation_not_cancellable` | operation kind without cancellation, `cancellable: false`, or already finished |
| 409 | `operation_still_running` | wiping away a running operation |
| 409 | `operation_not_retryable` | kind without retry, or operation did not fail |
| 409 | `payload_not_expected` / `payload_already_delivered` | payload to the wrong operation |
| 409 | `port_in_use` | port already belongs to a server of this panel |
| 409 | `port_unavailable` | a foreign process holds the port |
| 409 | `port_pool_exhausted` | nothing free left in the pool |
| 409 | `primary_allocation` / `already_primary` | deleting the primary port resp. setting it again |
| 409 | `allocation_limit` | more than 8 allocations per server |
| 409 | `property_is_panel_owned` | `server-port` or `query.port` |
| 409 | `properties_unsupported` | proxy without `server.properties` |
| 409 | `loader_change_needs_wipe` | family change without `content_policy: "wipe_mods"` |
| 409 | `modpack_not_linked` / `modpack_already_linked` | |
| 409 | `backup_limit_reached` | `max_backups` reached |
| 409 | `backup_not_restorable` / `backup_not_downloadable` | `status != "done"` |
| 409 | `nothing_to_retry` | the most recent operation did not fail |
| 409 | `log_file_in_use` | `logs/latest.log` and the server is running |
| 409 | `console_buffer_empty` | crash analysis on an empty buffer |
| 409 | `external_services_disabled` | an admin turned outgoing calls off |
| 409 | `username_taken` / `already_member` | |
| 409 | `email_taken` | the address already sits on an account or an open application; **only** on admin paths (12.3, 12.5) |
| 409 | `user_has_servers` / `servers_running` / `last_admin` | deleting a user |
| 409 | `user_busy` | another administrative action on this user is running |
| 409 | `role_unlimited` | the account is a panel admin and has no limits (12.8) |
| 409 | `system_user_not_ready` | system user missing or faulty |
| 409 | `registration_disabled` | sign-up is closed (20.1) |
| 409 | `invalid_state` | the application is still unconfirmed and cannot be approved (20.6) |
| 409 | `mail_not_configured` | no Resend key or no sender address (19.2) |
| 409 | `mail_no_link_base` | a mail with a link, but no panel address (19.10) |
| 409 | `no_email_address` | this account has no address anything could go to |
| 409 | `drive_not_configured` | the operator has not set up Google Drive (22.2) |
| 409 | `drive_not_connected` | this account has no Drive connected |
| 409 | `drive_already_linked` | this account already has a Drive connected |
| 409 | `drive_has_backups` | disconnecting without `?files=` while Drive backups exist |
| 409 | `backup_lives_in_drive` | this backup lies in the user's Drive; `message` carries the link (22.19) |
| 409 | `target_not_allowed` | the operator's rule does not allow this backup target (22.10) |
| 410 | `token_expired` | confirmation token expired; a new one can be requested (20.9) |
| 413 | `file_too_large` | body over `max_upload_bytes`, `max_bytes` or the modpack limit |
| 413 | `archive_too_large` | unpacking limits exceeded |
| 413 | `log_too_large` | unpacking ceiling hit while reading a `.gz` |
| 415 | `unsupported_media_type` | wrong `Content-Type` |
| 415 | `unsupported_file_type` | neither `.jar` nor `.zip` nor `.mrpack` |
| 415 | `unsupported_archive` | not a readable ZIP |
| 422 | `eula_not_accepted` | `eula_accepted` is `false` |
| 422 | `unknown_loader` | loader not in the catalog |
| 422 | `unsupported_game_version` | loader does not know this game version |
| 422 | `no_compatible_version` | no build for loader and game version |
| 422 | `unresolvable_dependency` | required dependency cannot be resolved |
| 422 | `invalid_modpack` | `modrinth.index.json` missing or unreadable |
| 422 | `command_empty` / `command_too_long` / `command_invalid` | console command |
| 422 | `log_not_text` | file not readable as UTF-8 |
| 429 | `rate_limited` | our own brake; `Retry-After` in seconds |
| 429 | `too_many_attempts` | sign-in brake |
| 429 | `upstream_rate_limited` | Modrinth, mclo.gs, Resend or Google throttled us |
| 429 | `mail_rate_limited` | too many mails of the same kind to the same address (19.10) |
| 429 | `mail_quota_reached` | our own daily amount is used up (19.10) |
| 500 | `internal` | everything else, including `io_error` |
| 502 | `upstream_unavailable` | foreign source does not answer; `message` names it |
| 502 | `mail_key_rejected` | Resend does not know the key or it may not send (19.11) |
| 502 | `mail_sender_rejected` | Resend does not take this sender or recipient (19.11) |
| 502 | `mail_refused` | Resend rejected the mail; permanently (19.11) |
| 502 | `mail_upstream` | Resend had an error; will be retried (19.11) |
| 502 | `mail_unreadable` | Resend answered in a form we do not understand |
| 502 | `drive_unavailable` | Google does not answer, or answers differently than documented |
| 507 | `no_space` | `ENOSPC`, `EDQUOT` or too little room for the planned file |
| 507 | `drive_quota_exceeded` | the user's Drive is full; **not** to be retried (22.15) |

### 1.8 Pagination

Three forms, each with a reason. There is no fourth.

| Form | where | why |
|---|---|---|
| `limit` + `offset`, response with `total` resp. `next_offset` | `GET /admin/users`, `GET …/audit-log`, `GET …/console/logs` | stable, rarely changed lists; the audit log is bound on top of that — `audit-log.ts:142-143` expects `next_offset`, `null` on the last page |
| `limit` + `before` (ULID), descending by `id` | `GET /operations`, `GET …/operations` | the ULID rises with time and keeps rising within the millisecond (1.3), a second sort column is unnecessary |
| `page_size` + `after` (name of the last entry), ascending by byte sequence | `GET …/files/list` | a running server keeps writing while you page; a positional cursor would let entries appear twice or not at all |

Deliberately **without** pagination, each with a hard ceiling instead of silent truncation:
`GET /servers` (dozens of servers, the interface searches client-side over the whole field),
`GET …/content` (2,000 items, then `truncated: true`), `GET …/backups` (capped by
`max_backups ≤ 50` and `history ≤ 20`), `GET …/allocations` (at most 8), `GET /loaders` (ten),
`GET …/game-versions` and `…/builds` (builds capped at 500, then `truncated: true`).

### 1.9 One world per server

`world_id` appears in **no** path, **no** body and **no** WS message. The shared components demand
one anyway: `composables/server-backups-queue.ts:23` (`enabled: !!worldId.value`),
`layouts/shared/server-settings/pages/properties.vue:396` and `.../advanced.vue:282`
(`enabled: worldId !== null`), `installation.vue:246`.

**Decision:** `ModrinthServerContext.worldId` is `ref("default")` — a fixed string, not a ULID, so
it is visible at once that it addresses nothing. The client adapter (5.5) swallows the argument
and cuts `/worlds/default` out of the Archon paths. An error code `world_not_found` is not needed
and not handed out, because no value ever reaches the backend.

`serverFull.worlds` stays empty; `serverFull` is constantly `null` (15.1, line 4).

### 1.10 Roles

**Panel role** `admin | user`, holds for the whole panel (`docs/PLAN.md:308`).
**Server role** `owner | editor | viewer`, holds per server. The two are independent
(`docs/PLAN.md:311-312`). In the interface the type is called `ServerAccessRole`
(`components/servers/access/types.ts:5`) and has exactly these three values.

Ten of Modrinth's fifteen bits are used (`composables/server-permissions.ts:15-32`); bit names of
our own are impossible, because `parsePermissionString` silently discards unknown names
(`:45-52`). `SUPPORT_AGENT` and the four `INFRA_*` fall away (`docs/PLAN.md:93-94`).

| Role | Bits |
|---|---|
| `owner` | `SERVER_ADMIN` |
| `editor` | `BASE_READ`, `POWER_ACTIONS`, `EXEC_COMMANDS`, `FILES_WRITE`, `SETUP`, `BACKUPS`, `ADVANCED` |
| `viewer` | `BASE_READ`, `POWER_ACTIONS` |

The mapping is not free: `apiPermissionsToAccessRole`
(`components/servers/access/permissions.ts:6-23`) derives the role back out of the bits, and
`hasServerPermission` treats `SERVER_ADMIN` as a short circuit before every individual check
(`server-permissions.ts:71-79`). A panel admin gets `current_user_permissions = "SERVER_ADMIN"` on
**every** server, without a membership row, and therefore does not appear in the member list.

**Wire format of the mask: a string**, bit names separated by ` | `, e.g.
`"BASE_READ | POWER_ACTIONS"`; empty = no permissions. Checked: `parsePermissions`
(`server-permissions.ts:56-59`) calls `value.split('|')` on anything that is not a number — a JSON
array would run into a `TypeError` there. The adapter sets
`current_user_permissions: mask as unknown as number` when building the vendor server object; in
the `computed` the cast would have no effect, because the field is already declared as `number`
there. Numbers are out, because `BASE_READ` = `1<<63`.

---

## 2. The permission matrix

`✓` allowed, `–` forbidden. The server roles hold per server; the panel admin has `SERVER_ADMIN`
on every server and therefore stands at `✓` everywhere.

### 2.1 Server-related endpoints

| # | Endpoint | Rule | viewer | editor | owner | Panel admin |
|---|---|---|---|---|---|---|
| 4.1 | `GET /servers` | session, filtered by `BASE_READ` | ✓ | ✓ | ✓ | ✓ (`scope=all`) |
| 4.2 | `POST /servers` | session; `owner_id`/`port` admin only | ✓ | ✓ | ✓ | ✓ |
| 4.3 | `GET /servers/:id` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 4.4 | `PATCH /servers/:id` | `ADVANCED` | – | ✓ | ✓ | ✓ |
| 4.5 | `DELETE /servers/:id` | owner or panel admin | – | – | ✓ | ✓ |
| 4.6 | `POST /servers/:id/power` | `POWER_ACTIONS` | ✓ | ✓ | ✓ | ✓ |
| 4.7 | `GET /servers/:id/ws` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 5.1 | `GET /operations` | session, filtered by `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 5.2 | `GET /servers/:id/operations` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 5.3 | `GET /servers/:id/operations/:op_id` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 5.4 | `POST …/operations/:op_id/cancel` | by operation kind (5.6) | – | partly | ✓ | ✓ |
| 5.5 | `POST …/operations/:op_id/dismiss` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 5.6 | `POST …/operations/:op_id/retry` | `SETUP`, on `backup_*` `BACKUPS` | – | ✓ | ✓ | ✓ |
| 5.7 | `PUT …/operations/:op_id/payload` | `SETUP` | – | ✓ | ✓ | ✓ |
| 6.1 | `POST …/console/command` | `EXEC_COMMANDS` | – | ✓ | ✓ | ✓ |
| 6.2 | `POST …/console/clear` | `EXEC_COMMANDS` | – | ✓ | ✓ | ✓ |
| 6.3 | `POST …/console/crash-analysis` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 6.4 | `GET …/console/logs` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 6.5 | `GET …/console/logs/content` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 6.6 | `DELETE …/console/logs` | `FILES_WRITE` | – | ✓ | ✓ | ✓ |
| 7.2 | `GET …/files/meta` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 7.3 | `GET …/files/list` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 7.4 | `POST …/files/create` | `FILES_WRITE` | – | ✓ | ✓ | ✓ |
| 7.5 | `POST …/files/move` | `FILES_WRITE` | – | ✓ | ✓ | ✓ |
| 7.6 | `DELETE …/files` | `FILES_WRITE` | – | ✓ | ✓ | ✓ |
| 7.7 | `GET …/files/content` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 7.8 | `PUT …/files/content` | `FILES_WRITE` | – | ✓ | ✓ | ✓ |
| 7.9 | `POST …/files/extract` | `FILES_WRITE` | – | ✓ | ✓ | ✓ |
| 8.1 | `GET …/content` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 8.2 | `GET …/content/modpack/contents` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 8.3 | `POST …/content/enable` | `SETUP` | – | ✓ | ✓ | ✓ |
| 8.4 | `POST …/content/disable` | `SETUP` | – | ✓ | ✓ | ✓ |
| 8.5 | `POST …/content/delete` | `SETUP` | – | ✓ | ✓ | ✓ |
| 8.6 | `POST …/content/update` | `SETUP` | – | ✓ | ✓ | ✓ |
| 8.7 | `POST …/content/install` | `SETUP` | – | ✓ | ✓ | ✓ |
| 8.8 | `POST …/content/upload` | `SETUP` | – | ✓ | ✓ | ✓ |
| 8.9 | `POST …/content/dependents` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 8.10 | `POST …/content/modpack/install` | `SETUP` | – | ✓ | ✓ | ✓ |
| 8.11 | `POST …/content/modpack/update` | `SETUP` | – | ✓ | ✓ | ✓ |
| 8.12 | `POST …/content/modpack/unlink` | `SETUP` | – | ✓ | ✓ | ✓ |
| 8.13 | `GET …/content/game-version/preview` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 8.14 | `POST …/content/game-version` | `SETUP` | – | ✓ | ✓ | ✓ |
| 9.1 | `GET …/properties` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 9.2 | `PATCH …/properties` | `ADVANCED` | – | ✓ | ✓ | ✓ |
| 9.3 | `GET …/startup` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 9.4 | `PATCH …/startup` | `ADVANCED` (+ budget); `startup_command` admin only | – | ✓ | ✓ | ✓ |
| 9.6 | `GET …/allocations` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 9.7 | `POST …/allocations` | `ADVANCED`; `port` admin only | – | ✓ | ✓ | ✓ |
| 9.8 | `PATCH …/allocations/:port` | `ADVANCED` | – | ✓ | ✓ | ✓ |
| 9.9 | `DELETE …/allocations/:port` | `ADVANCED` | – | ✓ | ✓ | ✓ |
| 9.10 | `PUT …/allocations/:port/primary` | `ADVANCED` | – | ✓ | ✓ | ✓ |
| 9.14 | `POST …/install` | `SETUP` | – | ✓ | ✓ | ✓ |
| 9.15 | `POST …/repair` | `SETUP` | – | ✓ | ✓ | ✓ |
| 9.16 | `POST …/reset` | `RESET_SERVER` | – | – | ✓ | ✓ |
| 9.17 | `POST …/reset-to-setup` | `RESET_SERVER` **and** panel admin | – | – | – | ✓ |
| 10.1 | `GET …/backups` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 10.2 | `POST …/backups` | `BACKUPS` | – | ✓ | ✓ | ✓ |
| 10.3 | `PATCH …/backups/:backup_id` | `BACKUPS` | – | ✓ | ✓ | ✓ |
| 10.4 | `DELETE …/backups/:backup_id` | `BACKUPS` | – | ✓ | ✓ | ✓ |
| 10.5 | `POST …/backups/bulk-delete` | `BACKUPS` | – | ✓ | ✓ | ✓ |
| 10.6 | `POST …/backups/:backup_id/restore` | `BACKUPS` | – | ✓ | ✓ | ✓ |
| 10.7 | `POST …/backups/:backup_id/retry` | `BACKUPS` | – | ✓ | ✓ | ✓ |
| 10.8 | `GET …/backups/:backup_id/download` | `BACKUPS` | – | ✓ | ✓ | ✓ |
| 10.9 | `GET …/backups/schedule` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 10.10 | `PUT …/backups/schedule` | `BACKUPS` | – | ✓ | ✓ | ✓ |
| 10.11 | `GET /modrinth/v0/backups/:id/download` | `BACKUPS` | – | ✓ | ✓ | ✓ |
| 11.1 | `GET …/members` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 11.2 | `POST …/members` | `MANAGE_USERS` | – | – | ✓ | ✓ |
| 11.3 | `PATCH …/members/:user_id` | `MANAGE_USERS` | – | – | ✓ | ✓ |
| 11.4 | `DELETE …/members/:user_id` | `MANAGE_USERS` **or** yourself | ✓ (self) | ✓ (self) | ✓ | ✓ |
| 11.5 | `POST …/members/:user_id/reinvite` | `MANAGE_USERS` | – | – | ✓ | ✓ |
| 11.9 | `GET …/audit-log` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 18.7 | `GET …/playit` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 18.8 | `POST …/playit` | owner or panel admin | – | – | ✓ | ✓ |
| 18.9 | `DELETE …/playit` | owner or panel admin | – | – | ✓ | ✓ |
| 22.9 | `GET …/backups/target` | `BASE_READ` | ✓ | ✓ | ✓ | ✓ |
| 22.10 | `PUT …/backups/target` | `BACKUPS` | – | ✓ | ✓ | ✓ |

On **22.9/22.10**: where a backup goes is a matter for whoever makes backups, not for the owner —
unlike 18.8, because a target opens no port to the internet. It does come about in the **owner's**
Drive, though, even when an editor switches it over; whoever has none connected gets
`409 drive_not_connected`.

On **5.4**: `unarchive` → `FILES_WRITE` (`FileOperationAdmonition.vue:110` checks exactly that),
`backup_create`/`backup_restore` → `BACKUPS` (`ServerPanelAdmonitions.vue:316`), `server_create` →
`SETUP`, `server_delete` → owner or panel admin. All remaining kinds cannot be cancelled (5.6) and
answer `409 operation_not_cancellable`.

On **5.5**: dismissing deliberately demands less than cancelling. The foreign component checks no
permissions on dismiss (`ServerPanelAdmonitions.vue:382-386`), on cancel it does; a viewer would
otherwise get a 403 on every click and the message would stay put.

On **18.8/18.9**: a public address is not a matter for the editor, even though they may write
files and restart the server. It puts the server on the open internet, and that is the owner's
decision — the same reasoning as with 4.5. The tunnel always comes about on the **owner's**
playit.gg account, even when a panel admin presses the button; if the owner has none connected,
18.8 answers `409 playit_not_configured`.

On **8.3–8.8**: writing content demands `SETUP`, not `FILES_WRITE`. Modrinth checks the same bit
for exactly these actions (`ref/…/content.vue:134,174` via `canSetup`). The role `editor` contains
both, so the difference only shows up with future special roles.

### 2.2 Panel-related endpoints

| # | Endpoint | signed in | Panel admin |
|---|---|---|---|
| 3.1 | `POST /auth/login` | without sign-in | without sign-in |
| 3.2 | `POST /auth/logout` | ✓ | ✓ |
| 3.3 | `GET /me` | ✓ | ✓ |
| 3.4 | `POST /me/password` | ✓ | ✓ |
| 3.5 | `GET /users/search` | ✓ | ✓ |
| 8.15 | `GET /modrinth/*path` | ✓ | ✓ |
| 9.5 | `GET /java-runtimes` | ✓ | ✓ |
| 9.11 | `GET /loaders` | ✓ | ✓ |
| 9.12 | `GET /loaders/:loader/game-versions` | ✓ | ✓ |
| 9.13 | `GET /loaders/:loader/game-versions/:v/builds` | ✓ | ✓ |
| 11.6 | `GET /invitations` | ✓ (own) | ✓ (own) |
| 11.7 | `POST /invitations/:id/accept` | ✓ (own) | ✓ (own) |
| 11.8 | `POST /invitations/:id/decline` | ✓ (own) | ✓ (own) |
| 12.1 | `GET /admin/host` | – | ✓ |
| 12.2 | `GET /admin/users` | – | ✓ |
| 12.3 | `POST /admin/users` | – | ✓ |
| 12.4 | `GET /admin/users/:user_id` | – | ✓ |
| 12.5 | `PATCH /admin/users/:user_id` | – | ✓ |
| 12.6 | `DELETE /admin/users/:user_id` | – | ✓ |
| 12.7 | `GET /admin/users/:user_id/limits` | – | ✓ |
| 12.8 | `PUT /admin/users/:user_id/limits` | – | ✓ |
| 12.9 | `POST /admin/users/:user_id/system-user/retry` | – | ✓ |
| 12.10 | `GET /admin/settings` | – | ✓ |
| 12.11 | `PUT /admin/settings` | – | ✓ |
| 18.1 | `GET /playit` | ✓ (own) | ✓ (own) |
| 18.2 | `POST /playit/claim` | ✓ (own) | ✓ (own) |
| 18.3 | `GET /playit/claim` | ✓ (own) | ✓ (own) |
| 18.4 | `DELETE /playit/claim` | ✓ (own) | ✓ (own) |
| 18.5 | `DELETE /playit` | ✓ (own) | ✓ (own) |
| 18.6 | `POST /playit/agent/restart` | ✓ (own) | ✓ (own) |
| 18.10 | `GET /admin/playit` | – | ✓ |
| 18.11 | `DELETE /admin/playit/:user_id` | – | ✓ |
| 19.2 | `GET /admin/mail` | – | ✓ |
| 19.3 | `PUT /admin/mail` | – | ✓ |
| 19.4 | `DELETE /admin/mail/key` | – | ✓ |
| 19.5 | `POST /admin/mail/test` | – | ✓ |
| 19.6 | `GET /admin/mail/outbox` | – | ✓ |
| 19.7 | `GET /admin/mail/outbox/:id/content` | – | ✓ |
| 19.8 | `POST /admin/mail/outbox/:id/retry` | – | ✓ |
| 19.9 | `GET /admin/mail/preview/:kind` | – | ✓ |
| 20.1 | `GET /auth/options` | without sign-in | without sign-in |
| 20.2 | `POST /auth/register` | without sign-in | without sign-in |
| 20.3 | `POST /auth/verify-email` | without sign-in | without sign-in |
| 20.4 | `POST /auth/verify-email/resend` | without sign-in | without sign-in |
| 20.5 | `GET /admin/registrations` | – | ✓ |
| 20.6 | `POST /admin/registrations/:id/approve` | – | ✓ |
| 20.7 | `POST /admin/registrations/:id/reject` | – | ✓ |
| 21.1 | `POST /auth/password-reset` | without sign-in | without sign-in |
| 21.2 | `POST /auth/password-reset/verify` | without sign-in | without sign-in |
| 21.3 | `POST /auth/password-reset/confirm` | without sign-in | without sign-in |
| 21.4 | `POST /admin/users/:user_id/password-reset` | – | ✓ |
| 22.3 | `GET /drive` | ✓ (own) | ✓ (own) |
| 22.4 | `POST /drive/link` | ✓ (own) | ✓ (own) |
| 22.5 | `GET /drive/link` | ✓ (own) | ✓ (own) |
| 22.6 | `DELETE /drive/link` | ✓ (own) | ✓ (own) |
| 22.7 | `DELETE /drive` | ✓ (own) | ✓ (own) |
| 22.8 | `POST /drive/check` | ✓ (own) | ✓ (own) |
| 22.11 | `GET /admin/drive` | – | ✓ |
| 22.12 | `PUT /admin/drive` | – | ✓ |
| 22.13 | `DELETE /admin/drive/credentials` | – | ✓ |
| 22.14 | `DELETE /admin/drive/:user_id` | – | ✓ |

**138 endpoints** in total: 136 ordinary HTTP endpoints under `/api/v1/`, plus the WebSocket
upgrade (4.7) and the compatibility alias (10.11). The thirty-one new ones are eight for mail
delivery (19), seven for sign-up (20), four for resetting a password (21) and twelve for Google
Drive (22).

Eight of them check **no** session (20.1–20.4, 21.1–21.3, plus 3.1). That is new for this panel:
up to this round there was nothing behind `/api/v1/` a stranger could reach except sign-in. Each of
those eight therefore needs a brake of its own (20.11, 21.6) — the sign-in brake (`auth/brake.rs`)
counts failed attempts on an account and does not fit here.

Until the rebuild to "one account per user" the eleven playit endpoints stood in **neither** of
these two tables and lived in `docs/PLAYIT.md` 8 alone. That gap is exactly what made the
confusion possible that a panel admin connects one account for everybody. They now stand here and
in detail in section 18; `18.n` is the same as `PLAYIT.md` `8.n`.

---

## 3. Session and your own account

### 3.1 `POST /api/v1/auth/login`

Request `LoginRequest`. Response `200` with `Me` (as in 3.3) and `Set-Cookie: craft_session=…`.

Errors: `401 invalid_credentials` (also on an unknown name — no difference on the outside),
`429 too_many_attempts` after ten failed attempts per account **and** per sender IP in 15 minutes,
lockout 15 minutes; `400 invalid_request`.

Two further responses since section 20, and their order is the contract: `403 email_unverified`
and `403 approval_pending` come **after** the password check. Whoever does not know the password
gets `401 invalid_credentials`. Otherwise the two codes would be a directory of names of open
applications. The cost stays the same too: if 3.1 finds no account, it checks the application's
argon2, and if it does not find that either, the one from `verify_against_nobody` (20.8). The brake
runs before that, unchanged.

### 3.2 `POST /api/v1/auth/logout`

No body. Response `204`, deletes the cookie (`Max-Age=0`) and the session row. Without a cookie
also `204` — signing out is idempotent. Open WebSockets of this session close with `4401`.

### 3.3 `GET /api/v1/me`

Response `200` with `Me`.

`email` is the account's address or `null` — accounts created by hand have none. It is always a
**usable** address: either its owner clicked the link (20.3) or an admin entered it (12.3, 12.5).
There is therefore no field "confirmed yes/no" and no column for it either; the unconfirmed state
lives in `registrations` and nowhere else. `origin` says how the account came about —
`"registration"` means somebody signed themselves up. `UserRef` (3.5) gets **neither** of the two:
an address is not something the search for invitation recipients may hand out.

`limits` feeds forms, `usage` feeds the displays; `usage.memory.limit_mib` stands there a second
time so a bar can be drawn from **one** object.

`limits` is `null` when no limit holds for this account (today: every panel admin, 12.7). Then
`usage.memory.limit_mib`, `usage.cpu.limit_cores`, `usage.pids.limit` and `usage.disk.limit_mib`
are `null` too; the fields do not disappear, so that a reader who forgets the case sees "no limit"
and not a missing key. There is no form then. `usage.memory.allocated_mib` is the sum of the
`memory_mib` of all your own servers, whether or not they are running (`docs/PLAN.md:320-322`) —
that is exactly the value that bounds the memory slider in the creation wizard
(`docs/PLAN.md:332`).

`usage.*.used_*` comes from the cgroup files (`memory.current`, `cpu.stat`, `pids.current`) and is
cached for 5 seconds; `measured_at` says how old the value is. `used_cores` needs two measuring
points — if the metrics tick stands still, the first value after a pause is `0.0`.

`usage.disk` is measured, not promised: `servers_bytes` is a walk over the account's server
directories, `backups_bytes` the sum of the `size_bytes` of its backups, `used_bytes` both
together. That costs a directory walk, hence a window of 60 seconds and a background run that
keeps it warm; `measured_at` holds for this too.

`usage.disk.complete` is `false` when a directory was closed to the panel during that walk. The
game process owns its tree and may create folders in it that the group `craftpanel` cannot enter
(`docs/PLAN.md:196-205`) — WorldEdit puts its language files down as `drwx--S---`, and
`Files.createTempDirectory` creates `0700`, without anyone meaning any harm. What lies inside
occupies the disk all the same, but is in none of the three numbers: they are a **lower bound**
then.

A closed directory is therefore not accepted but **opened**: the measuring machinery calls
`chown-tree` for `<account>/servers`, because root gets in everywhere and this command resets the
subtree to the shape from `PLAN.md:196-205` anyway — afterwards it counts a second time. Once, not
in a circle. If it stays closed even then (helper unreachable, error in the file system),
`complete: false` stays: the interface says "at least" instead of naming a number it does not
have, and the disk barrier from 12.7 **refuses everything new** — with `disk_limit_reached` when
even the lower bound is above the limit, otherwise with `disk_usage_unknown`. A limit you get
under with `chmod 0700` would be none. `true` means: everything counted.

### 3.4 `POST /api/v1/me/password`

Request `ChangePasswordRequest`. Response `204`. Side effect: **all other** sessions of the user
are discarded and their WebSockets closed with `4401`; the calling one gets a fresh cookie.

Second side effect since section 21: **all open reset tokens of this account are discarded**
(21.8). Whoever changes their password themselves thereby invalidates a link they requested
earlier. Otherwise an old mail opens an account the owner has just taken back. If the account has
an address, a `password_changed` mail goes out; if that fails, it stays at `204` (19.14).

Errors: `403 wrong_password`, `400 weak_password` (minimum length 10, no further rules),
`400 invalid_request`.

### 3.5 `GET /api/v1/users/search`

Query: `query` (from one character on), `limit` ≤ 25. Response `200` with `UserSearchResponse`.
Prefix search on the user name, case-insensitive. The response contains **only** ID, name, avatar
— no role, no limits, no servers.

Serves `GrantAccessModal.searchUsers` (`components/servers/access/GrantAccessModal.vue:195`,
debounced 250 ms, `:412`). Without this endpoint there is no invitation.

---

## 4. Servers

### 4.1 `GET /api/v1/servers`

Query: `scope` = `visible` (default) or `all`; `all` for panel admins only
(`docs/PLAN.md:358`), otherwise `403 forbidden`. Response `200` with `ServerListResponse`.

No pagination, no server-side search: the interface searches client-side with Fuse over
`['name','loader','game_version','game','owner.username']` and needs the complete list for it.
`users` delivers the display name of foreign owners; the split "Your servers" / "Shared with you"
is done by the list itself via `server.owner_id === session.user_id`.

**Live behavior.** There is no list-wide WebSocket. As long as at least one operation is running,
the list polls `GET /api/v1/operations?state=active` every **five seconds** and reloads itself as
soon as an operation reaches a final state; when nothing is running, the poll rests. That is
**one** poll, not two.

### 4.2 `POST /api/v1/servers`

Permission: session. `owner_id` and `port` may only be set by a panel admin
(`docs/PLAN.md:350-354`). Request `CreateServerRequest`. Response `201` with
`CreateServerResponse` and `Location: /api/v1/servers/<id>`.

**The response comes before anything is downloaded.** Within the request only what works without
the network happens: check the budget, take the port, hand out a ULID, write the database row,
create the directory, write the operation row. From the response on, the server is visible through
`GET /api/v1/servers/:id`, its WebSocket reachable and the operation `server_create` in the
database. That is the complete answer to the question of how an operation should report progress
when its server did not exist at the moment of the click — see 13.2.

The **system user** does not come about here: it hangs on the panel user and is created when the
account is created (`docs/PLAN.md:138-141`, helper command `create-user`).

**Budget and port are handed out in one transaction.** Two simultaneous requests from the same
user would otherwise read the same allocated sum and both be allowed, or they grab the same free
port. Checking and inserting run in one SQLite transaction; the port column carries a `UNIQUE`
constraint whose violation turns into `409 port_in_use`. Resolving the version against the foreign
source happens **before** the transaction, because it is network traffic and must not hold a lock.

`content.kind`:

* `loader` — `loader`, `game_version`, `loader_version` (`null` = newest stable build).
* `modpack_project` — `project_id`, `version_id`.
* `modpack_upload` — `file_name`, `file_size`; the file follows afterwards through 5.7.

`properties` is literally what the wizard builds
(`components/flows/creation-flow-modal/creation-flow-context.ts:524-541`). The server name comes
from `config.worldName` (`:140`).

What is measured is always the **owner's** budget, not the caller's. If a panel admin creates for
a limited account and the server does not fit into its budget, `warnings: ["memory_overcommitted"]`
comes instead of the error. For a server that belongs to a panel admin themselves there is neither
this warning nor `budget_exceeded` nor `over_limit`: they have no budget that could be exceeded
(12.7).

Errors: `400 invalid_request` (name empty or > 64 characters, `memory_mib` < 512),
`403 forbidden` (`owner_id`/`port` from a non-admin), `409 port_in_use`,
`409 port_pool_exhausted`, `409 budget_exceeded`,
`409 over_limit`, `409 disk_limit_reached`, `422 eula_not_accepted`, `422 unknown_loader`,
`422 unsupported_game_version`, `502 upstream_unavailable` (only while resolving from a cold
cache; otherwise it moves into the operation and becomes
`error.code = "upstream_unavailable"` there).

### 4.3 `GET /api/v1/servers/:id`

Permission: `BASE_READ`. Response `200` with `Server`. Stays reachable during `status: "deleting"`
too, so that an open detail page notices the end.

### 4.4 `PATCH /api/v1/servers/:id`

Permission: `ADVANCED` (`server-settings/pages/general.vue:358` aborts without
`canUseAdvancedSettings`, and that is exactly this bit, `server-permissions.ts:97`).
Request `UpdateServerRequest` — `name` and/or `update_channel`. Response `200` with `Server`,
plus a WS message `server` to everyone connected.

`update_channel` stands here because it would otherwise be homeless: `GET …/content` reads it, but
no area had a write path. An endpoint of its own for it would be one line of payload on a path of
its own. It is written in the settings area under "General".

A **change** of the channel sets `updates_checked_at` to `null` — the channel decides what counts
as an update, and the old finding must not survive the next six hours (8.16). The next 8.1 read
request thereby counts as expired and checks again. If the same channel comes in again — the
settings page sends it along with every rename — the timestamp stays put.

Errors: `400 invalid_name`, `400 invalid_request`.

### 4.5 `DELETE /api/v1/servers/:id`

Permission: owner or panel admin. Deliberately **no** server bit: an editor must not destroy the
owner's work.

Query: `keep_backups` (`true` by default). Response `202` with `OperationAccepted`
(`kind: "server_delete"`).

Precondition: `power_state` is `stopped` or `crashed`, otherwise `409 server_running` — no silent
shooting down, whoever wants to kill presses Kill first. Check and delete run under the same server
lock as the power wishes (4.6).

Afterwards: the server disappears from `GET /api/v1/servers` **immediately** and stands at
`status: "deleting"`; `GET /api/v1/servers/:id` and the WebSocket stay reachable until the end of
the operation. Only when `server_delete` is finished do the sockets close with `4404`. Port and
budget become free when the operation begins, the directory is deleted afterwards in the
background.

**A delete that fails brings the server back into the list.** It disappears while the run is
running, and only that long: if `server_delete` ends in `failed`, the row stands in
`GET /api/v1/servers` again — still with `status: "deleting"`, because it is still on its way out,
and the reason stands on the failed operation. Without this rule the state arises that happened to
one user: the server is gone from the list, its directory is still lying there,
`GET /api/v1/me` counts it under `disk.used_bytes` and `servers.total`, and it can no longer be
seen to try again. **The way to the second attempt is the same request**: `DELETE` on the same
server creates a new operation. 5.6 deliberately does not retry `server_delete` — the old run is
over, the new one is a delete like any other: it takes the row out of the list again while it
runs, and hands it back again if it fails too. What hides the row is the run that is going, not
one that once went. Otherwise the server would stay visible during the whole second attempt,
because the first is still standing there.

What the delete does before it deletes: **a `chown-tree` on the server tree**
(`docs/PLAN.md:196-205`). The game runs as `craft-<owner>` and creates directories the panel
cannot get into — WorldEdit unpacks its languages as `drwx--S---`, and a `DELETE` failed on that.
The same helper command that creation uses resets owner **and** permissions (`2770`/`0660`) and
runs as root, so it gets in everywhere. Its failing does not abort the run: a helper that stays
silent must not turn every delete into an error, and whether it was enough is said by the delete
itself.

Errors: `409 server_running`, `409 server_busy`. Errors of the operation: `delete_failed`,
`permission_denied`, `no_space` (5.11).

### 4.6 `POST /api/v1/servers/:id/power`

Permission: `POWER_ACTIONS`. Request `PowerRequest`, response `202` `PowerResponse`. The response
is only the acknowledgment of receipt; what is binding is the WS message `state` that goes to
everyone connected right afterwards — to the trigger too.

```
on request:
  start    stopped | crashed              → starting → running
  stop     running | starting             → stopping → stopped
  restart  running | starting             → stopping → starting → running
  kill     starting | running | stopping  → stopped

on its own:
  exit code 0                             → stopped
  exit code ≠ 0 or OOM kill               → crashed
```

`stop` writes the command `stop` to standard input and is therefore a request. After
`panel_settings.stop_grace_seconds` (default 60, 12.10) the same signal ladder as with `kill`
begins: SIGTERM, 10 s later SIGKILL. The grace period used to be handed to the supervisor and
thrown away there: a server that never executed the console command thus stayed in `stopping`
forever, and that is exactly what happened to one user. `stopping` now has an exit in every case.
An end forced this way comes by signal and looks like a crash from the outside; what is reported
is `stopped` all the same, because a stop is what was asked for — the intent weighs more than the
cause of death, only a real OOM kill stays `crashed` (13.4).

`kill` sends SIGTERM to the **process group** of the game and 10 s later SIGKILL, the same grace
period as between the two signals of a `stop`. The group is signalled and not the single PID, so
what the server started itself is signalled too: wrapper scripts, proxies, everything that would
otherwise keep holding the port. A child that takes a session of its own with `setsid` escapes;
the only way there would be the cgroup, and that belongs to the account, not to the server
(`docs/PLAN.md:229-234`) — a kill through it would hit the owner's remaining servers along with it.

What a user notices of this: the response is `stopping` without `target`, the console keeps
writing, and a server that still hears its signals saves the world and is gone after a few
seconds. So `kill` is not an immediate cutting off; the 10 s are only waited out for whoever
ignores SIGTERM. Afterwards we report **`stopped`, never `crashed`**: `powerState === 'crashed'`
triggers the crash analysis (6.3), and for a shooting down you triggered yourself that would be
wrong. `oom_killed` stays `false` too, even when this account has really failed on memory once
before (13.4).

Restriction: the ladder sits in the supervisor, not in the panel. A supervisor that was already
running before the update still carries the old code — with it `kill` stays without a signal and
`stop` without a grace period, until its server has been restarted once. Restarting the panel does
not help there — precisely because running servers survive it and the same supervisor signs on
again (`docs/PLAN.md:231-233`).

The server holds **one lock per server**, under which the state is read, the transition checked
and the wish given to the supervisor. Without it the check would let two `start`s through and two
Java processes on the same port would come about. Deleting (4.5) takes the same lock.

Errors: `409 invalid_power_transition`, `409 server_busy` (a locking operation is running, 5.6),
`409 server_broken` (on `start`), `409 budget_exceeded`/`409 over_limit` (the admin lowered the
limit below what is allocated: what runs keeps running, what is stopped no longer starts,
`docs/PLAN.md:364-366`).

### 4.7 `GET /api/v1/servers/:id/ws`

WebSocket upgrade, permission `BASE_READ`, plus the `Origin` check from 1.2. Complete protocol in
section 13.

The upgrade is **not** refused merely because the server was never started, has no directory with
content, or carries `status: "installing"` resp. `"deleting"`. It hangs on the database row, not
on a process. Without this rule the creation flow breaks (13.2).

If the check before the upgrade fails (no session, no `BASE_READ`, foreign `Origin`), the upgrade
is **accepted anyway and then closed** — with `4401`, `4403` resp. `4403`. On a refused upgrade a
browser only gets a bare `error` event without a status, and `isWsAuthIncorrect` could not be told
apart from "network gone".

---

## 5. Operations — the one progress model

**Every** long-running operation of the panel uses this model: creating and deleting a server,
installing a loader, repairing, resetting, installing and updating a modpack, installing and
updating content, switching the game version, creating and restoring a backup, unpacking an
archive, fetching a Java runtime. One table, one type, one WS message, one set of endpoints. No
area brings a progress of its own.

**The rule for all other sections:** an endpoint that triggers a long-running operation answers
`202 Accepted` with `{ "operation": { … } }` (`OperationAccepted`). Some endpoints hang a field on
it — `total` at 8.6, `planned`/`skipped` at 8.7 — but never a second operation object. If a
writing endpoint runs into a lock, it answers `409 server_busy`; which lock takes hold stands in
the `message` only, machine-readably you get it from `GET …/operations`.

### 5.1 `GET /api/v1/operations`

All operations across all servers the caller may see. That is how the server list gets by without
a second WebSocket.

Query: `state` (`active` default | `all`), `server_id` (repeatable), `limit` (100, max 200),
`before` (ULID). Response `200` `AllOperationsResponse`.

`busy_reasons_by_server` always lists all visible servers with a lock, independent of the
pagination — the number is bounded by the number of servers on the machine.

### 5.2 `GET /api/v1/servers/:id/operations`

Query: `state` (`active` default | `all`), `include_dismissed` (`false`, with the one exception
from 5.5), `limit` (50, max 200), `before` (ULID). Response `200` `OperationListResponse` — the
same snapshot the socket sends, for the moment before the connection is opened and for everything
without a socket.

**`revision`.** Both ways — HTTP and socket — carry the same counter, which rises by one on every
state change of an operation of this server. The provider discards every snapshot with a
`revision` smaller than or equal to the last applied one, no matter where it comes from. Without
that a slow HTTP response overwrites a newer socket message, and an operation that has just
finished would stand there as "running" until something changes the next time.

### 5.3 `GET /api/v1/servers/:id/operations/:op_id`

Response `200`: the `Operation` object without an envelope. For callers that wait for the end on
purpose after a `202`, without opening the socket — scripts, tests, and the fallback path of the
content provider after a dropped connection (five-second tick, gives up after ten minutes without
progress).

### 5.4 `POST /api/v1/servers/:id/operations/:op_id/cancel`

No body. Response `200` with the operation — in state `cancelled`, or still `ongoing` when the
cancellation is requested but not carried out. What the caller should rely on is the WS message,
not this response.

Permission and cancellability by operation kind (5.6). Errors: `409 operation_not_cancellable`.

Effect on the three cancellable kinds:

* `unarchive` — files already written stay put. Unwinding would be more dangerous with a
  half-unpacked modpack than the state itself.
* `backup_create` — delete the partial file, the backup row disappears; `save-on` is sent in
  every case.
* `server_create` — sensible, because the user may have picked the wrong modpack and the server
  is not worth anything yet. Triggered from our own creation page, not from the banner.
* `backup_restore` — only in state `queued`. A half-unpacked server directory is worse than an
  unpacking carried to the end; in `ongoing` therefore `409 operation_not_cancellable`.

### 5.5 `POST /api/v1/servers/:id/operations/:op_id/dismiss`

Wipes away a **finished** operation: sets `dismissed_at`, after which it appears neither in the
snapshot nor in 5.2 (except with `include_dismissed=true`). No body, response `204`.

Server-side, not only in the browser. Then what was wiped away stays wiped away, even after the
page is rebuilt. This one endpoint serves Modrinth's
`backups_queue_v1.ackCreate`/`ackRestore` at the same time: a backup's `should_prompt` is exactly
`dismissed_at === null`. Wiping away repeatedly is idempotent (`204`), because
`ServerPanelAdmonitions.vue:364-380` calls without checking when dismissing in bulk.

**One exception, and only this one: a `server_delete` that stands at `failed` while its server
still carries `status: "deleting"`.** This operation stays in the snapshot and in 5.2, even when
wiped away. Wiping away means "read", not "never happened", and this run is not a past but the
state the server is in: it is the reason why the row stands in `GET /api/v1/servers` again (4.5),
and the page reads its notice text off it. Without the exception the notice falls back to "Being
deleted" after one click — on a server nobody is deleting any more. The socket is not a second way
there: 13.2 sends the snapshot on its own, and you cannot ask a socket for `include_dismissed`.

Errors: `409 operation_still_running`.

### 5.6 `POST /api/v1/servers/:id/operations/:op_id/retry`

Retries a **failed** operation with the same inputs, creating a **new** operation in the process
and wiping the old one away. Modrinth wires the banner's retry button to a fresh run as well. No
body, response `202` `OperationAccepted`.

Not possible with `unarchive` (the source file may be gone), `server_delete` and **`backup_*`** —
backups are retried through 10.7, because `backups_queue_v1.retry(serverId, worldId, backupId)`
passes the backup ID, not the operation ID.

Errors: `409 operation_not_retryable`, `409 server_busy`.

### 5.7 `PUT /api/v1/servers/:id/operations/:op_id/payload`

Delivers the payload of a waiting operation afterwards. Exactly one use case: the uploaded
`.mrpack` from 4.2. `Content-Type: application/octet-stream`, the body is the raw file — no
`multipart`, there is exactly one field, and the progress bar hangs on the XHR, not on the format.

The operation stays `queued` until the body is fully there. Response `202` `OperationAccepted`. A
waiting operation without a payload goes to `failed` after **15 minutes** with
`error.code = "payload_timeout"`.

The upload progress does **not** come over the WebSocket but out of the XHR's `progress` events
and fills `ctx.uploadState` (`providers/server-context.ts:66`, displayed by
`UploadAdmonition.vue:60-66`). The server knows nothing more precise about a running upload than
"the body is not over yet".

**The second bolt follows from exactly that.** Because the row only jumps to `delivered` when the
body is fully there, two browsers that deliver to the same waiting operation both get past
`payload_already_delivered` and write into *one* file, and both get `202`. So the process holds
the identifiers of the runs that are currently receiving in a set in memory, and the second upload
is refused while the first is coming in (`ops/mod.rs:252-262`, `ops/api.rs:272-276`). One panel per
machine, so this one set is the whole lock — the socket count from 13.6 is held the same way. The
bolt falls however the first upload ended.

Errors: `409 payload_not_expected`, `409 payload_already_delivered`, `413 file_too_large`,
`422 invalid_modpack`.

### 5.8 Kinds, locks, cancellability

The column "locks" is what the server enforces with `409 server_busy`; the column `busy_reason` is
what the interface sees. The two are deliberately congruent: because `busyReasons.length > 0` in
the foreign interface blanket-locks **all** power actions
(`components/servers/server-header/use-server-power-action.ts:39-44,52-60`, including Kill) and
through `isBusy` the file manager as well (`files-tab/layout.vue:307` and 24 further places) and
the content tab (`content-tab/layout.vue:283-286,419,483`), the grayed-out button and the refusal
would otherwise differ from each other.

| Kind | triggered by | `busy_reason` | locks | cancellable | server may run |
|---|---|---|---|---|---|
| `server_create` | 4.2 | `installing` | everything | yes | — |
| `server_delete` | 4.5 | `deleting` | everything | no | no |
| `install_loader` | 9.14 | `installing` | everything | no | no |
| `repair_content` | 9.15 | `installing` | everything | no | no |
| `reset_server` | 9.16 | `installing` | everything | no | no |
| `install_java` | 9.4 | `installing` | everything | no | yes |
| `install_modpack` | 8.10, 8.11 | `installing` | everything | no | no |
| `change_game_version` | 8.14 | `installing` | everything | no | no |
| `install_content` | 8.7 | `syncing_content` | everything | no | yes |
| `update_content` | 8.6 | `syncing_content` | everything | no | yes |
| `backup_create` | 10.2 | `backup_creating` | everything | yes | yes |
| `backup_restore` | 10.6 | `backup_restoring` | everything | yes (`queued` only) | no |
| `unarchive` | 7.9 | **none** | nothing | yes | yes |

**`unarchive` sets no busy reason**, and therefore no file endpoint refuses with
`409 server_busy` during an unpack either. The reason is compelling: the file manager locks itself
out through `isBusy` (`ref/…/files.vue:53`), and that is exactly the page on which you want to see
the progress. Several `unarchive`s per server are serialized instead: the second one stays
`queued`, which `FileOperationAdmonition.vue:7` correctly shows as waiting. The price is named:
renaming or deleting in the same target directory during an unpack is a race whose outcome nobody
predicts.

**The installation operations cannot be cancelled.** `InstallingBanner` has no cancel button; it
knows "retry" on an error and "dismiss" (`InstallingBanner.vue:29-41`,
`ServerPanelAdmonitions.vue:214-221`). Offering a cancellation that no foreign component can
trigger would be dead code. The emergency exit is deleting the server.

### 5.9 Phases and how they map

`OperationPhase` has seven values; the interface knows four. The provider does the mapping, and
the four target values are Modrinth's spelling — `InstallingBanner.vue:187-196` compares them
literally, `ServerPanelAdmonitions.vue:179` checks for `'Analyzing'`.

| our `phase` | `SyncProgress.phase` | what for |
|---|---|---|
| `analyzing` | `Analyzing` | resolve the version, fetch the address and the checksum (0 → 0.05) |
| `installing_loader` | `InstallingLoader` | load the jar (0.05 → 0.60) |
| `verifying` | `InstallingLoader` | compare the checksum |
| `running_installer` | `InstallingLoader` | `--installServer` on NeoForge, Quilt, Forge |
| `installing_pack` | `InstallingPack` | read the `.mrpack`, unpack overrides, load files |
| `addons` | `Addons` | download content and lay it out |
| `writing_config` | `Addons` | `eula.txt`, `server.properties`, `-Xmx`, startup command (0.95 → 1) |

**The banner is invisible during `analyzing`** (`ServerPanelAdmonitions.vue:179`) — this phase must
therefore stay short and must contain no network traffic with progress.

Our API stays with lowercase and underscore, because `phase` also hangs on operations that never
reach the banner.

### 5.10 Busy reasons

`BusyReasonCode` has five values. The message IDs the provider builds from them are **not freely
chosen**: four of them are compared as strings in the foreign code, so that banner and warning do
not double up.

| Code | Message ID | where compared |
|---|---|---|
| `installing` | `servers.busy.installing` | `ServerPanelAdmonitions.vue:67,76`; `use-server-power-action.ts:27` |
| `syncing_content` | `servers.busy.syncing-content` | same place `:67,76`; `use-server-power-action.ts:28` |
| `backup_creating` | `servers.busy.backup-creating` | `ServerPanelAdmonitions.vue:72` |
| `backup_restoring` | `servers.busy.backup-restoring` | same place `:72` |
| `deleting` | `servers.busy.deleting` — new, from us | nowhere; falls into the general branch |

An unknown code is harmless (it appears as "Background task running",
`ServerPanelAdmonitions.vue:79-85`); a wrongly named one shows banner and warning at the same time.

**`busy_reasons` comes from the server**, not from a computation in the browser, and travels in
the same message as the operation list. Otherwise busy reason and list can drift apart. From it
the provider additionally derives `server.status = 'installing'`, because
`use-server-power-action.ts:23`, `installation.vue:205` and the server list read the status
separately. The backup part of `useServerBackupsQueue().busyReasons`
(`composables/server-backups-queue.ts:92-110`) is **not** fed in on top; otherwise there would be
two sources for the same lock.

### 5.11 Error codes of an operation

`error.code` is stable; `error.message` is what `InstallingBanner` sees as
`ContentError.description`. Where Modrinth's banner translates a text anyway
(`InstallingBanner.vue:150-176`), we send exactly that text and get the translation for free.

| `code` | `step` | `message` |
|---|---|---|
| `unsupported_game_version` | `modloader` | `this version is not yet supported` |
| `invalid_version` | `modloader` | `the specified version may be incorrect` |
| `loader_install_failed` | `modloader` | `internal error` |
| `modpack_no_primary_file` | `modpack` | `no primary file` |
| `modpack_install_failed` | `modpack` | `failed to install modpack` |
| `invalid_modpack` | `modpack` | own text |
| `checksum_mismatch` | `download` | own text |
| `upstream_unavailable` | `download` | own text |
| `no_space` | `filesystem` | own text |
| `permission_denied` | `filesystem` | own text — `EACCES`/`EPERM`, the panel was not allowed |
| `delete_failed` | `filesystem` | own text — a tree could not be removed (4.5) |
| `disk_limit_reached` | `filesystem` | own text |
| `archive_corrupted` | `filesystem` | own text |
| `invalid_path` | `filesystem` | own text |
| `interrupted_while_applying` | `filesystem` | own text |
| `restore_interrupted` | `filesystem` | own text |
| `safety_backup_failed` | `filesystem` | own text |
| `drive_not_connected` | `filesystem` | own text — the target is `drive`, the account has none connected |
| `drive_revoked` | `filesystem` | own text — Google withdrew the access (22.17) |
| `drive_file_missing` | `filesystem` | own text — the file is no longer in the Drive |
| `drive_quota_exceeded` | `filesystem` | own text — the user's Drive is full |
| `drive_unavailable` | `download` | own text — Google was not reachable |
| `payload_timeout` | `internal` | own text |
| `panel_restarted` | `internal` | own text |
| `timeout` | `internal` | own text — no progress for over 10 minutes |
| `cancelled_by_user` | `internal` | own text |

The state is called `failed` and not `error`, because six checkpoints of the foreign interface
write `state?.startsWith('fail')` (`FileOperationAdmonition.vue:3,94`;
`ServerPanelAdmonitions.vue:182-193`). An error state without this prefix would stand there
forever as a blue "Extracting …".

`cancelled` **is sent**; the provider filters it out of `activeOperations`, because
`FileOperationAdmonition.vue:94` counts only `done` and `fail*` as finished and a `cancelled`
would otherwise stand there as a permanent notice that cannot be clicked away. A cancelled
operation therefore disappears from the interface at once, the expected behavior.

### 5.12 Restarting the panel, and cleaning up

**Principle: nothing is resumed, except the delete.** Our operations have no checkpoints; we are
not building a journal per operation for runs of one minute.

When the panel starts, in this order:

1. Every row in `queued` or `ongoing` goes to `failed` with `error.code = "panel_restarted"`,
   `finished_at = now` and **without** `dismissed_at` — the user should see that something was
   broken off.
2. Exception: `server_delete` is resumed. Deleting is repeatable, and a half-deleted server has no
   value that would have to be preserved. What is resumed is whatever is `queued` or `ongoing`; a
   delete that already stands at `failed` is **not** started again on its own. A run that tries the
   same thing at every start and fails is not progress — the server stands in the list again for
   that (4.5) and waits for a human's second attempt.
3. Cleanup happens per kind (table below).
4. Every working directory `<server directory>/.craftpanel-tmp/<op_id>/` without a running
   operation is deleted, orphaned ones without a database row too.
5. Finished operations older than **seven days** are deleted — except for the same one that 5.5
   does not let you wipe away: a `server_delete` at `failed` whose server still stands at
   `deleting` stays put as long as it stands there. It is not a past but the reason for a state
   that is still here. The audit log is a different table and stays (180 days, 11.9).

**The working directory is the whole cleanup rule.** Every operation that creates files puts them
under `.craftpanel-tmp/<op_id>/` first and only pushes them into place with `rename` at the end.
The file manager hides `.craftpanel-tmp`. As long as nothing was moved, a crash leaves exactly
nothing behind.

| Kind | cleanup after a restart in the middle of the run |
|---|---|
| `server_create` | delete the working directory, empty the server directory (it can contain nothing of the user's), `status: "broken"`, `flows.intro: true`. Port and system user stay taken |
| `install_loader`, `repair_content`, `reset_server`, `install_modpack`, `change_game_version` | delete the working directory, `status: "broken"` — a half-swapped loader starts into an incomprehensible crash |
| `install_content`, `update_content` | delete the working directory. Every file is loaded to completion on its own and then moved, so it is either fully there or not at all. The server stays `available` |
| `install_java` | delete the working directory, server unchanged |
| `backup_create` | delete the started archive and the backup row, send `save-on`. Nothing changed on the server |
| `backup_restore` | `status: "broken"` with `error.code = "restore_interrupted"`. Being honest is better than passing off a broken state as sound |
| `unarchive` | see below |
| `server_delete` | resume |

**The half-unpacked archive.** An `unarchive` has two parts, and the operation row carries
`applied_at` for that. **Before** `applied_at` it unpacks into `.craftpanel-tmp/<op_id>/`; a crash
leaves nothing behind. **After** `applied_at` the entries are moved into the target; that takes
milliseconds, but with many entries it is not atomic. If the panel crashes exactly there, a subset
lies in the target — **we do not delete these files**, the operation ends in `failed` with
`interrupted_while_applying`. Deleting the user's files because we are unsure is the worse mistake.

### 5.13 Maximum number of simultaneous operations

Per server everything is serialized. Panel-wide there is one queue with a width the admin can set
(`max_concurrent_operations`, default 2, 12.10) — ten simultaneous modpack installations on ten
servers would otherwise saturate the line. A waiting operation stands in `queued` while it waits,
which the interface correctly shows as waiting.

---

## 6. Console

### 6.1 `POST /api/v1/servers/:id/console/command`

Permission: `EXEC_COMMANDS`. Request `SendCommandRequest`, response `204`.

The command is **not** returned in the response. It comes back as an ordinary console line over the
WebSocket, to everyone connected including the sender: `[15:04:22] [Panel/INFO]: > say hello`. That
way there is exactly one path on which lines enter the buffer, and no duplicate rendering; without
the echo, a user running a command that produces no output would be looking at an empty field,
because the input clears immediately (`components/base/BaseTerminal.vue:237-243`).

**Over HTTP, not over the socket** (unlike Modrinth): that way permission, state and length errors
come back as a status code with a stable `error`. The contract returns `void`
(`console/providers/console-manager.ts:14`), so the layout cannot evaluate an error anyway, but
our provider needs a response it can evaluate for the notification. The price: two commands sent in
quick succession can in theory swap order; the backend writes to stdin under a mutex.

Errors: `409 server_not_running`, `422 command_empty`, `422 command_too_long` (> 8192 bytes UTF-8),
`422 command_invalid` (`\n`, `\r` or control characters — otherwise several commands get smuggled
into one), `429 rate_limited` (more than 20 commands in 10 s per user and server).

Audit log: `console_command_executed` with `{ "command": "…" }`.

### 6.2 `POST /api/v1/servers/:id/console/clear`

Permission: `EXEC_COMMANDS`. No body, response `204`. Clears the ring buffer in memory and sends
`console_cleared` to everyone connected. **`logs/latest.log` is not touched**, `seq` keeps running.
Audit log: `console_cleared`, without metadata.

### 6.3 `POST /api/v1/servers/:id/console/crash-analysis`

Permission: `BASE_READ`. Request `CrashAnalysisRequest` (`source`: `latest_log` default |
`buffer`), response `200` `CrashAnalysisResponse`.

The response is the one from `POST https://api.mclo.gs/1/analyse`, **trimmed** to the fields of
`Mclogs.Insights.v1.InsightsResponse` — `success` and `entries` fall away. Measured on 2026-08-12:
`entries` holds every parsed log entry, 202 entries and 33 KB of JSON for a 20 KB log; untrimmed,
the response would be as large as the file we were trying not to push through the browser in the
first place. The layout reads only `analysis.problems[].message` and `.solutions[].message`
(`console/layout.vue:136-147`).

**Through our backend, not from the browser**, because otherwise the browser would first have to
download `latest.log` only to upload it again straight away. The backend has the file, cuts it back
to the last 2 MiB at a line boundary and forwards only that (mclo.gs truncates at 10 MiB and 25 000
lines itself, `GET /1/limits`, checked).

`name` and `version` are **nullable**: the real API returns `null` there as soon as it does not
recognize the loader or the version; Modrinth's `string` (`mclogs/types.ts:36,39`) is too narrow,
and a Rust struct with `String` failed on the first real response.

`analysis.problems` may be empty; the provider then sets `crashAnalysis` to `null`. That is the
normal case — for "FAILED TO BIND TO PORT", "Could not reserve enough space for object heap" and
the missing EULA, an empty list came back on 2026-08-12 in each case.

Cache 10 minutes, key for `latest_log` from (`server_id`, mtime, length), for `buffer` from
(`server_id`, `seq` of the last line, line count). Required, not optional: mclo.gs throttles per
IP, and with us every user of a panel comes from one IP. On `429` from mclo.gs we remember the
block per panel for 60 seconds instead of knocking again.

The call is triggered by the provider when `power_state` jumps to `crashed`, so not after `kill`
(4.6).

Errors: `404 log_file_missing`, `409 console_buffer_empty`, `409 external_services_disabled`,
`429 upstream_rate_limited`, `502 upstream_unavailable`.

### 6.4 `GET /api/v1/servers/:id/console/logs`

Permission: `BASE_READ`. Query `limit` (200, 1…500), `offset` (0). Response `200`
`LogFileListResponse`.

Included is whatever ends in `.log`, `.log.gz` or `.txt` in `logs/` and whatever ends in `.txt` in
`crash-reports/`; subdirectories and symbolic links are skipped. Sorted by `modified_at`
descending, `logs/latest.log` always first.

**Why there is paging although the interface cannot page.** A Minecraft server rotates
`latest.log` on every start and on top of that at midnight, and nobody clears it out; after a year
those are four-digit numbers. The combobox builds **every** option into the DOM
(`components/base/Combobox.vue:122-127`, no virtualization). So: sort, then cap. `total` is the
number before capping, `truncated` is `offset + files.length < total`. If you need older files, use
the file manager.

As long as the server is running, the provider hides `logs/latest.log` from the selection: the
live console shows the same thing.

### 6.5 `GET /api/v1/servers/:id/console/logs/content`

Permission: `BASE_READ`. Query `file` (required). Response `200` `LogFileContentResponse`.

`content` is unpacked plain text; the server unpacks `.gz`. At most the **last 25 000 lines or
8 MiB** are delivered, and the cut is always at the **front** — the end is the interesting part —,
then `truncated: true`. The 25 000 are no accident: that is exactly how many mclo.gs accepts, and
the share button sends the displayed content there (6.7).

**Unpacking needs a ceiling.** `logs/` holds more than what the server writes there: every file
gets there through the file manager, and 200 KB of gzip can unfold into gigabytes. Unpacking runs
streaming into a moving window; at **512 MiB** of unpacked bytes the operation aborts with
`413 log_too_large`.

Paths follow the rules from 7.1 (N1–N7, `openat2` with `RESOLVE_BENEATH`); in addition the
normalized path must begin with `logs/` or `crash-reports/` and must not lie one directory deeper.

Errors: `400 invalid_path`, `403 forbidden_path`, `404 log_not_found`, `413 log_too_large`,
`422 log_not_text`.

### 6.6 `DELETE /api/v1/servers/:id/console/logs`

Permission: **`FILES_WRITE`** — deleting is a file access, not a console permission; a viewer may
read logs but not clear them away. Query `file`. Response `204`. Deleting uses `unlinkat` on the
parent descriptor, so a link as a link and never its target.

Errors: `400 invalid_path`, `403 forbidden_path`, `404 log_not_found`, `409 log_file_in_use`
(`logs/latest.log` and the server is running).

The `message` in the 409 case has to be usable: it lands in front of the user unchanged, because
the provider has to throw errors of this call as a **string** — `console/layout.vue:414-417` shows
`typeof err === 'string' ? err : 'Unknown error.'`, an `Error` object yields "Unknown error." there.

Audit log: `file_deleted` with `{ path }` — a console event name of its own would be "Unknown
event", because `parseAuditEvent` only renders the catalog from 11.9.

### 6.7 No endpoint: sharing to mclo.gs

Sharing runs **straight from the browser**, because the layout hard-wires it: `handleShare` builds
the text from `ctx.logLines` and calls `client.mclogs.logs_v1.create(content)`
(`console/layout.vue:422-443`); this module has `api: 'https://api.mclo.gs'` and `skipAuth: true`
fixed in the code. What gets shared is only what search and level filter leave over anyway.

Three consequences: we have to provide a `ModrinthClient`, otherwise the injection alone throws
(`console/layout.vue:130`). This client must configure **no headers of its own** — measured on
2026-08-12: `POST https://api.mclo.gs/1/log` answers with `access-control-allow-origin: *`, but the
**`OPTIONS` preflight comes back without any `access-control-*` headers at all**; it works only
because `URLSearchParams` plus `application/x-www-form-urlencoded` is a simple CORS request. One
single extra header through `config.headers` forces the preflight, and the button dies. And: our
browser buffer is the size limit, hence 25 000 lines / 8 MiB instead of Modrinth's 500 000.

If the machine has no outside access, or the admin has switched off outbound services (12.10), the
provider sets `shareDisabled`. The button cannot be hidden away (`ConsoleActionButtons.vue:26`
renders it as soon as there are lines).

### 6.8 What the server needs to know about console lines

**No level detection in the backend.** `detectLogLevel` (`console/composables/log-level.ts:5-14`)
determines the level from the line text; `LogLine` knows only `text` and `level` anyway
(`console/types.ts:3-6`). We send raw text, no `level`, no timestamp of our own and no `stream`
(stdout and stderr are merged; Modrinth does not evaluate a stream marker).

**The start of the line is sacred.** Two places check `/^\[\d{2}:\d{2}:\d{2}\]/` — the grouping of
continuation lines (`composables/server-console.ts:14`) and the block highlighting of errors
(`console/composables/log-highlight-addon.ts:24`). **Nothing** may be put in front of the Minecraft
timestamp, or the rendering of stack traces falls apart.

Lines the panel produces itself carry the same shape and a level marker in the tag:

```
[15:04:22] [Panel/INFO]: > say hello
[15:04:25] [Panel/INFO]: Server process started (pid 21044)
[15:07:01] [Panel/ERROR]: Server process exited with code 1
```

Rules for the producer: only complete lines (a remainder is held until `\n` arrives, at the latest
250 ms); `\r\n` and `\r` fall away, a leading BOM as well; ANSI sequences and control characters
other than tab are removed (the layout wraps the whole line in SGR codes, an embedded `\x1b[0m`
ended the coloring midway); lines over 8 KiB are shortened and get ` [truncated]` appended.

The output of a second-wave installer (NeoForge, Quilt, Forge) goes into **this** stream, not into
a message of its own. Otherwise there would be two paths for the same thing.

---

## 7. Files

### 7.1 Path model and jail — applies to every endpoint in this section and to 6.5/6.6

**Root.** Every server has exactly one directory,
`/var/lib/<panel>/users/<user-id>/servers/<server-id>` (`docs/PLAN.md:150-158`). All `path` values
are relative to it; there is no way to name anything outside it.

**Wire format.** POSIX, `/` as the only separator. On Linux a backslash is an ordinary character in
a file name and **not** a separator. A leading `/` is allowed and means nothing; `""` and `"/"` are
the root. This tolerance is mandatory, because the layout produces both forms: breadcrumbs deliver
`plugins/config` without one (`files-tab/layout.vue:391`), the move dialog `/plugins` with one
(`components/modals/FileMoveItemModal.vue:94`).

**Responses always deliver `path` with a leading `/`.** `prefetchFile` stores the cache under
`item.path`, `readFile` reads it back under the path normalized to `/` — without the leading slash
the prefetch never hits.

| No. | Rule | Violation → |
|---|---|---|
| N1 | Percent-encoding is decoded **exactly once** (HTTP layer). `%252e%252e%252f` becomes the file name `%2e%2e%2f`, not `../` | — |
| N2 | no null byte anywhere in the path | 400 `invalid_path` |
| N3 | valid UTF-8 | 400 `invalid_path` |
| N4 | split on `/`, discard empty segments and `.` | — |
| N5 | a `..` segment is **not resolved, it is rejected** — including `a/../b` | 400 `invalid_path` |
| N6 | segment ≤ 255 bytes, relative path ≤ 4096 bytes, depth ≤ 64 | 400 `path_too_long` |
| N7 | the result is a list of segments; empty list = root | — |

On N5: resolving `..` lexically is the classic trap — `a/../b` is *not* `b` exactly when `a` is a
symbolic link. The interface never produces `..` by itself; only the free-text field of the move
dialog could deliver it, and there an error message is the right answer.

**Enforcement in the file system.** Normalization alone says nothing about links.

1. When the server is created, a directory descriptor of the root is opened
   (`O_PATH|O_DIRECTORY`) and held. **No code ever assembles path strings.**
2. Every access goes through `openat2(root_fd, relpath, RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS)`.
   The kernel enforces the boundary without a race between check and use.
3. `RESOLVE_BENEATH`, **not** `RESOLVE_IN_ROOT`: `IN_ROOT` would quietly bend a link to
   `/etc/passwd` into `<root>/etc/passwd`; we want to see an error.
4. For creating, writing, deleting and move destinations, `O_NOFOLLOW` goes on the last segment.
5. Deleting a link removes the link, never its target (`unlinkat` on the parent descriptor).
6. Only regular files may be read, written and downloaded — FIFOs block on open, device files
   deliver an endless amount: `400 not_a_regular_file`.
7. `RESOLVE_NO_XDEV` is **not** set; a separate mount point per server should stay possible.

**The way *to* the root is the gap `RESOLVE_BENEATH` does not close.** It guards the root you give
it, so whoever picks the root decides what the jail is worth. And the two directories above it
belong to the account that runs the game (`users/<id>` and `users/<id>/servers`,
`craft-<id>:craftpanel 2770`, `docs/PLAN.md:150-158`): it may push its own server directory aside
and put a link in its place. That is why the way in is walked segment by segment starting at
`<data_dir>/users` — the last piece of the path no server account can change — every step with
`O_NOFOLLOW`, and a swapped segment ends as `ELOOP` (`files/jail.rs:35-39,122-126`
`ON_THE_WAY_IN`, `settings/disk.rs:158-192`). `RESOLVE_BENEATH` on its own is not enough here for
another reason too: it lets a link *to below* `users/` through, and that is all the attacker needs,
because it puts another account's tree under their own server ID (`api/files.rs:1220-1261`).

Measured twice before the way in was walked like this:

* a `server.properties` that pointed at the panel's `config.toml` — a `GET` handed the panel
  configuration to anyone with `BASE_READ`, a `PATCH` overwrote it (`settings/disk.rs:4-8`);
* a server directory that had been replaced by a link — `200 OK`, and another account's
  `server.properties` carried the attacker's text, because the panel reaches every tree through
  the `craftpanel` group (`api/settings.rs:2057-2062`).

On kernels before 5.6 there is no `openat2`. The fallback is one `openat` per segment with
`O_NOFOLLOW`, which therefore rejects **every** link instead of only the ones that lead out:
stricter than the contract, never looser (`files/jail.rs:178-180`).

And one thing must **not** come out as `forbidden_path`: a tree the game process has nested three
hundred levels deep. In this area `forbidden_path` is the sentence "the path has left the tree",
and that is exactly what it has not done here — it is longer than a path may be (N6), and it says
so. Telling a caller that their own `world` points outside would teach them the wrong idea about
their server (`files/jail.rs:410-412`, counter-check `api/files.rs:1494-1497`). How deep a
recursive delete or a copy across a file system boundary goes at all is a limit of its own: deeper
than the 64 from N6, so that a tree the panel laid down itself can always be removed again, and
shallow enough that no archive can run us down the stack (`files/jail.rs:48-50`).

**Why this is the most important paragraph in this area.** Through the `craftpanel` group the panel
service reaches every user directory and `panel.db` with the password hashes
(`docs/PLAN.md:151-167`). A plugin on user A's server runs as their system user and may place a
link there. Without a kernel-side jail, `ln -s /var/lib/<panel>/panel.db …/logs/latest.log` is
enough and one click on "Download" hands out the panel database.

**Serving foreign content.** 7.7 delivers bytes uploaded by the user from the panel's own origin.
Hence **always** `Content-Disposition: attachment`, `Content-Type: application/octet-stream` and
`X-Content-Type-Options: nosniff`. Otherwise an uploaded `.html` or `.svg` is stored XSS against
your own panel. The image preview does not suffer: it fetches a blob and creates an object URL.

### 7.2 `GET /api/v1/servers/:id/files/meta`

Permission: `BASE_READ`. Response `200` `FilesMetaResponse`. The provider queries it once per
server; it fills `basePath` (menu item "Copy full path", `FileTableRow.vue:211`) and gives upload
and editor their upper limits instead of wiring them into the frontend.

### 7.3 `GET /api/v1/servers/:id/files/list`

Permission: `BASE_READ`. Query `path` (`/`), `after`, `page_size`. Response `200`
`ListDirectoryResponse`.

Sorting does **not** happen on the server: the interface sorts and searches across the whole array
itself (`composables/file-sorting.ts:26-75`, `file-search.ts:9-14`); a page is a transfer unit, not
a display unit. Transfer order is the byte sequence of the name.

The provider fetches pages as long as `has_more` holds, up to at most **20 000** entries.
**Prefetching fetches exactly one page** — `prefetchDirectory` fires after only 150 ms of hovering
over a row; if it were allowed to page on, a mouse pointer over `world/` would trigger twenty
requests nobody asked for.

`type` comes from `lstat`, without following the link: a link to a directory is `symlink`, not
`directory`. Otherwise deleting would set `recursive=true` and take the linked directory with it.
The price: the row is dead in the interface, because `selectItem` knows only `directory` and `file`
(`FileTableRow.vue:344-348`). `count` costs one extra `getdents` per subdirectory, but only for the
entries of the page being delivered.

Non-UTF-8 names are **listed** with a lossily converted name (U+FFFD) so the user sees them; read
and write accesses to them answer `400 non_utf8_name`, deleting is allowed if the lossy form is
unique within the directory. Otherwise there would be no way to get rid of garbage.

**The cursor from 1.8 is the name as *shown*, and that decides the sort order.** Ordering follows
the byte sequence of what goes over the wire, not the raw bytes on disk. Sort by the raw ones and
compare the returning cursor against them, and the caller gets a marker that sorts *before* the
very entry it came from: they page over that one entry forever, and everything behind it stays out
of reach. For the same reason two entries whose shown name is identical leave on the same page
together — every byte a name cannot spell becomes the same U+FFFD, and if the page split between
them, the second would sit behind a marker that already covers it and would never be seen again.
Those are exactly the ones that should be visible (`files/mod.rs:317-345`, counter-checks
`files/mod.rs:714-717` and `:734-738`).

Errors: `400 invalid_path`, `400 not_a_directory`, `403 forbidden_path`, `404 not_found`.

### 7.4 `POST /api/v1/servers/:id/files/create`

Permission: `FILES_WRITE`. Request `CreateItemRequest`, response `201` `CreateItemResponse` with
the finished `ApiFileItem`, so the provider can extend the list without a second request.

Only the **last** segment is created; if the parent directory is missing: `404 parent_not_found`.
New files are empty, directories get `0770` and inherit the `craftpanel` group through the setgid
bit.

The interface checks names against `^[a-zA-Z0-9-_.\s]+$` (file) and `^[a-zA-Z0-9-_\s]+$` (folder).
The server does **not** check the same thing: it forbids only `/`, null bytes, `.` and `..` —
otherwise existing files with umlauts could no longer be touched, and undo replays old names past
the dialog (`composables/file-undo-redo.ts:32-34`).

Errors: `400 invalid_path`/`invalid_name`, `403 forbidden_path`, `404 parent_not_found`,
`409 already_exists`, `507 no_space`.

### 7.5 `POST /api/v1/servers/:id/files/move`

Permission: `FILES_WRITE`. Serves `moveItem` **and** `renameItem`: a rename endpoint of its own is
unnecessary. Request `MoveItemRequest`, response `200` `{ "moved": true }`.

`destination` is the **full** target path including the name (`files-tab/layout.vue:453`). On a
rename the provider replaces the last segment of `source`. `source == destination` yields `200`
with no effect (undo/redo can produce that). Implemented as `renameat2` on the parent descriptors,
so atomic within the same file system; across file system boundaries it copies and then deletes.

If nothing may be overwritten, this `renameat2` carries `RENAME_NOREPLACE`: "does it exist already"
and "move it" are then **one** step and not two, between which someone can create the file. If the
kernel or the file system does not know the call, only the old way is left — look first, then move
(`files/jail.rs:333-334,361-362`). When copying across the file system boundary the reverse
applies: there everything is written with `O_EXCL`, so whatever may give way has to go first — the
`rename` would have replaced it in one move (`api/files.rs:322-323`).

Across file system boundaries the tree is handed back to the game identity beforehand
(`chown-tree`, `docs/PLAN.md:196-205`): every file gets copied and every directory gets deleted
into, and a folder the game process created with `0700` lets the panel do neither. If handing it
back does not succeed, that is logged; whether it worked anyway is what the copy itself says.

Errors: `400 invalid_path`/`invalid_move`, `403 forbidden_path`, `404 not_found`/
`parent_not_found`, `409 already_exists`, `409 file_not_accessible`, `507 no_space`.

### 7.6 `DELETE /api/v1/servers/:id/files`

Permission: `FILES_WRITE`. Query `path`, `recursive` (`false`). Response `204`.

The bulk delete button sends **one request per entry, without waiting for one another**
(`files-tab/layout.vue:579-584`). So the server has to tolerate several deletions in the same
directory in parallel, and "already gone" is not an error: on `not_found` we still answer `204`, so
a double click produces no red message.

Before a `recursive=true` on a **directory** the panel hands the tree back to the game identity
(`chown-tree`, `docs/PLAN.md:196-205`). Without that the deletion stops at the first folder the
game process closed off to the `craftpanel` group. A single file does not need it — it is unhooked
from its parent directory without anything being walked into. If handing it back fails, that is
logged and the deletion happens anyway; whether it worked is what the deletion itself says.

Errors: `400 invalid_path`, `403 forbidden_path`, `409 not_empty`, `409 file_not_accessible`.

### 7.7 `GET /api/v1/servers/:id/files/content`

Permission: `BASE_READ`. One endpoint for three contract methods: `readFile`, `readFileAsBlob`,
`downloadFile`. Query `path` (required), `max_bytes`, `download` (`0`/`1`, affects only the file
name in `Content-Disposition`).

Response `200` with the bytes, plus `Accept-Ranges: bytes`, `ETag` (`mtime-size`) and
`Cache-Control: private, no-cache`. `Range` is supported (`206`), so an interrupted download of a
large world file can be resumed; the interface does not use it, browsers do it on their own.

The provider sets `max_bytes = max_text_bytes` for `readFile` and `readFileAsBlob` and leaves it
out for `downloadFile`. Above `max_bytes`: `413 file_too_large`, **without** sending a body.

The server icon runs over the same endpoint: `GET …/files/content?path=/server-icon.png`.

Errors: `400 invalid_path`/`not_a_regular_file`, `403 forbidden_path`, `404 not_found`,
`413 file_too_large`.

### 7.8 `PUT /api/v1/servers/:id/files/content`

Permission: `FILES_WRITE`. Serves `writeFile` (editor) **and** `uploadFiles` (upload). Query
`path`, `on_conflict` (`fail` default | `overwrite`). Body: raw bytes,
`application/octet-stream`. **No multipart** — an `XMLHttpRequest` can send a `File` object
unchanged as the body and reports upload progress while doing so. Response `204`.

How it is written: `.<name>.part.<ulid>` goes into the same directory, `fsync`, then `renameat2`
onto the target name. That way there is no half-written `server.properties`. Part files older than
24 hours are cleared away by the service at start-up; they are **not** hidden — a hidden file that
uses up disk would be a lie to the user.

No `If-Match`: the `ETag` is delivered and an `If-Match` accepted, but never required. The editor
shows the same text for every error ("Save failed"), so the user would not learn why, and the
Minecraft server writes into its own files while someone has them open.

**Uploading** (`uploadFiles`): files **one after another**, one `PUT` each with
`path = target folder + '/' + file.name` and `on_conflict=fail`. One after another, because
`currentFileName` and `currentFileProgress` represent exactly one file in flight. The **target
folder is captured once at the call**, not read afresh from `currentPath` for every file:
`uploadFiles` returns no promise, uploading locks nothing, and nothing stops the user from
navigating on — without that capture the remaining files land in the wrong folder. On
`409 already_exists` the provider reports the file name and carries on with the next file; there is
no conflict dialog for uploads, and silently overwriting a `server.properties` would be the more
expensive mistake.

Errors: `400 invalid_path`/`invalid_name`, `403 forbidden_path`, `404 parent_not_found`,
`409 already_exists`, `409 disk_limit_reached`, `413 file_too_large`, `507 no_space`.

`disk_limit_reached` falls before the first byte is read: what `Content-Length` announces is
checked against the owner's free remainder (12.7). Without an announcement the only check is
whether there is any room left in the pot at all. A rejection leaves no half file behind.

### 7.9 `POST /api/v1/servers/:id/files/extract`

Permission: `FILES_WRITE`. Request `ExtractRequest`.

* `dry: true` → **synchronous** response `200` `ExtractDryRunResponse`. No operation is created.
  `files-tab/layout.vue:508` checks only `conflicting_files.length`; if the list is empty, the real
  extraction starts right away.
* `dry: false` → `202` `OperationAccepted` with `kind: "unarchive"`, `src` = archive path.

`target` is optional; **the default is the directory the archive is in**. Modrinth extracts into
the server root — that is modpack installation, not file management. The parameter stays in the
contract because modpack installation uses `target: "/"`. The interface always passes `override`
through as `true`; it collects the consent beforehand through the conflict dialog.

ZIP only; `.jar`, `.tar`, `.gz`, `.rar`, `.7z` yield `415 unsupported_archive`. The interface
offers "Extract" only for `.zip` anyway, and extracting a `.jar` is never what anyone wants.

Checks per archive entry, because they change outcomes:

1. The entry name is normalized like a `path` (7.1). `..`, absolute paths, null bytes, invalid
   UTF-8 → entry skipped, the operation ends `failed` with `error.code = "invalid_path"`.
2. Entries that are symbolic links or device files are skipped.
3. Sum of the uncompressed sizes above `max_extract_uncompressed_bytes` or entry count above
   `max_extract_entries` → `413 archive_too_large`, already in the dry run, because the values are
   in the ZIP's central directory.
4. Ratio of uncompressed to compressed above 200:1 with more than 64 MiB of output → abort with
   `error.code = "archive_corrupted"`.
5. Every entry is written as in 7.8 (part file, then rename), directories created beforehand.

Four sentences on these five checks, each of which went the wrong way round once while it was being
built:

* **N6 belongs to check 1.** An entry that nests deeper than a path may go is just as unusable a
  name as one that climbs out, so it is skipped, counted and named at the end. Stopping the run at
  it would leave everything before it in the working directory and everything after it unwritten,
  over a single bad name (`files/archive.rs:471-473,818-823`).
* **At most what the central directory promised is read.** That is exactly where a bomb hides: the
  header lies, and the data stream does not stop (`files/archive.rs:491-492`).
* **Check 3 is asked twice** — in the dry run as `413`, and once more by the run itself, because it
  reads the archive again, and between the two readings a second `PUT … on_conflict=overwrite` may
  have put a far bigger one at the same path. 5.11 has no code of its own for an upper limit;
  `no_space` is the one that means "no room for what is planned"
  (`files/archive.rs:74-76,429-433`).
* **The one `chown_tree` of the rule from `PLAN.md:205` starts at the shallowest segment of the
  target that does not exist yet.** A target the run creates itself brings its parent directories
  into the world with it, and those belong to the panel until someone says otherwise — a directory
  the game process may not enter is as bad as a file it cannot read
  (`files/archive.rs:362-366`, counter-check `api/files.rs:1744-1747`).

And the run's split in two is in 5.12: until `applied_at` everything lands in
`.craftpanel-tmp/<op_id>/`, and a crash leaves nothing behind; after that the entries are moved
into place one by one, and a crash leaves part of them standing. **That part is not cleared away**
— deleting someone's files because we are unsure is the worse mistake (`files/archive.rs:4-9`).

`conflicting_files` are at most **200** entries; above 100 the dialog switches to "Over 100 files
will be overwritten … here are some of them" anyway.

Errors: `400 invalid_path`, `403 forbidden_path`, `404 not_found`, `409 disk_limit_reached`,
`413 archive_too_large`, `415 unsupported_archive`, `507 no_space`.

`disk_limit_reached` takes the unpacked size from the archive's central directory, which the dry
run knows anyway — the only door that knows the number exactly in advance. A `dry: true` does not
check it: it writes nothing.

### 7.10 No file watching, and who shows the errors

There is **no** "file has changed" event. A running Minecraft server writes continuously; watching
the whole tree would be expensive and the interface would have nowhere to show it. Refreshing runs
through the button, after every write and when an `unarchive` transitions to `done` or `failed`.

**The button does not hang on the contract**, but on the layout property `showRefreshButton`
(`files-tab/layout.vue:288-291`). Mount `<FilePageLayout />` without `:show-refresh-button="true"`
and you ship a file manager with no way at all to reload by hand.

**Errors reach the user only if the provider shows them.** In seven places the layout does not
catch a rejected promise (`createItem`, `renameItem`, `moveItem` twice, `deleteItem` twice,
`downloadFile` — `layout.vue:426,435,455,471,484,498,582`); the only things caught are
`extractFile` and everything in the editor. Binding rule: **every contract method catches for
itself, reports through `addNotification` and returns a resolved promise.** Never pass one through.

From this follows a requirement on the application: the **notification provider is mandatory**.
`injectNotificationManager()` is called without a fallback value (`files-tab/layout.vue:293`,
`FileTableRow.vue:142`, `FileContextMenu.vue:85`, `FileEditor.vue:68`), and
`providers/create-context.ts:66` then throws: the entire files tab crashes on mount. Second
requirement: the `ModrinthClient`, otherwise the editor crashes when opened
(`FileEditor.vue:70`).

Deliberately not served: `showInstallFromUrl` (pulls in `FileUploadZipUrlModal`, which calls
`client.kyros.files_v0.extractFile`), `openInFolder` (only meaningful in the desktop program),
`downloadButtonLabel`, `uploadingLabel`, `canRestart`/`restartServer`, `canShareToMclogs`,
`busyWarning`. No directory download. No SFTP (`docs/PLAN.md:97`).

---

## 8. Content

### 8.1 `GET /api/v1/servers/:id/content`

Permission: `BASE_READ`. Query `refresh_updates` (`false`) — `true` kicks off the update check in
the background, the response does not wait for it. Response `200` `ContentListResponse`.

The one query the whole page is built from. It contains only the files the user installed **on top
of** a modpack — the modpack files come from 8.2. Otherwise the list would be unusable with a
200-mod modpack and the heading "Additional content" (`content-tab/layout.vue:794`) would be a lie;
Modrinth splits it the same way (`from_modpack=true|false`).

**No paging, and that is deliberate.** The layout cannot do it: `useContentFilters` counts the type
filters across **all** items (`composables/content-filtering.ts:73-84`), `useContentSearch`
searches the complete array, `ContentCardTable` renders everything. A partial delivery would
quietly falsify filters, counters and selection. Instead a hard upper limit of **2 000** items and
`truncated: true`, so the page does not lie in silence.

**No `busy` block.** `isBusy` and `busyMessage` come from `busyReasons` of the server context, fed
from the WS message `operations` (5.10). A second path for the same lock would be a source of
contradictions.

`content_type` is the label Modrinth derives from the loader (Paper/Purpur → `plugin`, Vanilla →
`datapack`, otherwise `mod`). We deliver the result along with it, because we know more loaders:
Paper, Folia, Purpur, Leaf and Velocity are plugin platforms, Vanilla is `datapack`,
Fabric/Quilt/Forge/NeoForge are `mod`.

`item.id` is a **ULID of our database row**, stable across renaming, updating and reinstalling the
same file. That is mandatory, not convenience: the layout's default would be
`file_path ?? file_name ?? id` (`content-tab/layout.vue:162`), and enabling/disabling renames the
file (`.disabled`). If the row ID changed with it, `useContentSelection` would discard the
selection on the next pass (`composables/content-selection.ts:16-26`). The provider sets
`getItemId: item => item.id`, and all bulk endpoints take this `id`, not file names.

**Two fields, two jobs.** `project_id` lives in our own row (`content_items.project_id`) and is
filled as soon as the panel could match the file to a Modrinth project; it hangs on no cache and on
no network call. `project` is the card from the cache of 8.16 (ID, slug, title, icon) and **may be
null even when `project_id` is set**: the cache can be empty or the entry expired.

More than a label hangs on this: the row button "Update available"
(`content-tab/layout.vue:644` → `updateItem` → `openUpdater`), "Switch version" and the modpack
update all fetch their version list through the same project ID. They take `project_id` for it, not
`project.id`. Read through the cache, the button did nothing while the cache was empty — no dialog,
no message. If `project_id` is null as well (a file dropped in by hand that nobody recognized), the
provider does not open an empty dialog, it **says so**.

`project: null` still demands a **substitute from the provider**: `ContentCardTableItem.project` is
not optional and `ContentCardItem.vue:148,161-166,187,364` reads `project.title` and
`project.icon_url` unchecked. Substitute:
`{ id: project_id ?? item.id, slug: project_id ?? item.id, title: item.file_name, icon_url: null }`
— with the project ID as the slug the row carries the file name but still links to Modrinth
correctly, because `/mod/<id>` resolves there. Who fills the cache and how long an existing row
waits for its title is in 8.16.

`date_added` comes from our database row; for files that never went through the panel (file
manager, shell) there is no row — then `mtime`.

`environment` comes from `Labrinth.Versions.v3.Version.environment`; only `client_only` and
`singleplayer_only` are read from it (`content-filtering.ts:10-14`). As long as the value is
missing, `null` — then the warning triangle stays off. **Modrinth answers on this field in two
forms**, measured on 2026-08-12 against the real service: v2 sends a **list**, v3 a string. Both
are read (only the first entry of the list — the interface checks one word), and on a v2 response
the two v2 fields that `environment` was folded from are read as well. Reading only one of the two
forms leaves the warning triangle off for the other
(`content/modrinth.rs:173-175,182-183,809-810`).

`truncated` counts what was **found**, not what is shown. With a linked modpack the largest part of
the array goes to 8.2; measuring the short list reported a full disk as complete
(`content/scan.rs:27-29`).

**The content directory itself also goes through the jail from 7.1 before it is opened.**
`read_dir` follows a link even though `metadata` on the entries does not: a `mods` that the game
process has swapped for a link is not a content directory, and without this step the list would
carry names, sizes and dates of other people's files. Links themselves are never content and are
never listed (`content/scan.rs:44-53`, counter-check `content/tests.rs:1623-1626`). A directory
that does not exist and one where the panel is turned away both come out as "nothing" — only the
log tells the two apart.

`locked` is `true` for the loader jar and the server core and drives
`canDeleteItem`/`canToggleItem`.

**One row per path, and that is why uploading the same file a second time keeps the old ID.**
`content_items` has a unique index over the path. So a fresh row would not merely be a lost
selection in the browser, but a failed `INSERT` — **after** the file on disk has already been
swapped: a `500` over a mod that has already been replaced. The same race the other way round as
well: a jar dropped in by hand, then the same project from Modrinth under the same file name. And
the disabled twin (`…​.disabled`) goes along with it — two jars would be two mods to the
loader (`content/mod.rs:387-390`, `content/store.rs:116-119`, counter-checks
`content/tests.rs:1882-1885,1912-1913,1941-1942`).

### 8.2 `GET /api/v1/servers/:id/content/modpack/contents`

Permission: `BASE_READ`. Response `200` `ModpackContentsResponse` — the same `ApiContentItem`,
filled into `ModpackContentModal.show(items)`. Errors: `409 modpack_not_linked`.

### 8.3–8.5 `POST …/content/enable` · `/disable` · `/delete`

Permission: `SETUP`. Three endpoints, one pattern: request `ContentIdsRequest` — **always a list**,
even for a single item. Response `200` `ContentMutationResponse`; partial success is possible and
is reported. On `/delete`, `file_name`, `file_path` and `enabled` are always `null`.

The provider only throws when **all** entries fail; otherwise it reports the individual errors as a
notification and reloads. `content-tab/layout.vue:496,514` expects a resolved `Promise` and reloads
afterwards anyway; throwing on a single failure would swallow the remaining successes.

Enabling and disabling appends the suffix `.disabled` or removes it, the same convention
Modrinth's desktop app reads. The `id` stays.

**Why the bulk form:** `bulk-operations.ts:12-39` only counts "Deleting 3/7…" upwards if the layout
loops item by item itself, with a 250 ms pause in between, and it does that only if we do **not**
supply the bulk functions. We supply them: deleting seven items is then one request instead of
seven plus six artificial pauses. The price is an indeterminate bar with the text "Deleting
content…".

If an item is a **folder** — a datapack, a plugin's own directory — the panel hands it back to the
game identity before deleting (`chown-tree`, `docs/PLAN.md:196-205`); otherwise `remove_dir_all`
would stop at the first subfolder the game process closed off. A single file does not need it. If
handing it back fails, that is logged and it is deleted anyway.

**And handing back stands at the error exit of every run in this section too.** A run that gives up
halfway has usually put something down already — the third of five mods lies in `mods/` when the
fourth does not download. These files belong to the panel until someone says otherwise, and the
game process cannot read them. So the tree goes back here as well, and only after that is the
failure written down (`content/mod.rs:1361-1366`, counter-check `content/tests.rs:1696-1699`).

Errors: `400 invalid_request` (empty list), `404 content_not_found`, `409 server_busy`. Per item
also `file_not_accessible` if the panel cannot get in even after that — this is not an `internal`,
it is a statement about the folder.

### 8.6 `POST /api/v1/servers/:id/content/update`

Permission: `SETUP`. Request `ContentUpdateRequest` in three forms: a single item with a chosen
target version, a selection with `version_id: null` (the server works it out), or
`{items: [], all: true}`. Response `202` `ContentUpdateResponse` — `OperationAccepted` plus
`total`.

`total` is the number of items the operation touches. The provider needs it because
`Operation.progress` is 0…1 and `BulkOperationStatus.progress` is a count
(`ContentSelectionBar.vue:55`: "Updating {progress}/{total}"). That is why version resolution runs
**synchronously before** the response; only that way is `total` right, and only that way are
`404 content_not_found` and `422 no_compatible_version` HTTP errors at all instead of operation
errors. The operation is the downloading and putting into place, nothing before it.

**The returned `Promise` has to stay open until the end of the operation.**
`content-tab/layout.vue:704-722` sets `isBulkOperating = true` before the `await`; for that whole
time a `beforeunload` handler and a confirmation on leaving the page are in place. The provider
resolves only once the operation reaches `done`, `failed` or `cancelled`; if the socket breaks off,
it falls back to 5.3 on a five-second beat and gives up with an error after ten minutes without
progress. Without this rule the page stays locked for good after a connection drop.

Errors: `404 content_not_found`, `409 server_busy`, `409 disk_limit_reached`,
`422 no_compatible_version`, `502 upstream_unavailable`.

`disk_limit_reached` is asked **before** version resolution and with `0`: an update puts the new
file down before the old one goes, so it is new bytes like 8.7 — but every entry replaces a file
that is already counted, and adding the plan's total on top would also reject an update that gets
smaller. So the only check is whether there is any room left in the owner's pot at all (12.7).

### 8.7 `POST /api/v1/servers/:id/content/install`

Permission: `SETUP`. Request `ContentInstallRequest` — project ID and optionally version ID,
nothing more. Response `202` `ContentInstallResponse` — `OperationAccepted` plus `planned` and
`skipped`.

**The server resolves dependencies.** Four reasons: the browser would have to fetch
`/v2/project/{id}/version` for every dependency and filter it itself; the backend needs the same
logic anyway for `.mrpack`, "Update all" and the game version change; only the backend reliably
knows what is on disk; and an operation that lives on the server side saves the whole apparatus of
`localStorage` and queue with which Modrinth's web solution catches "installation started, browser
window closed" (`utils/server-content-installing.ts:10-45`).

Resolution rules (a rebuild of `versionMatchesCompatibilityTarget`,
`utils/version-compatibility.ts:38-73`):

* `version_id` set → exactly this version, without a compatibility check. What the user picks
  explicitly, we install.
* `version_id` empty → the newest version whose `game_versions` contains our game version and
  whose `loaders` fit our loader; loader alias groups as in `version-compatibility.ts:3-6`
  (`paper`/`purpur`/`spigot`/`bukkit`, `neoforge`/`neo`); channel rules per
  `content-tab/utils/update-channels.ts:37-44`. `effectiveUpdateChannel` (`:16-24`)
  **raises** the default when the installed version is itself beta or alpha. For that the backend
  needs the `version_type` of the installed version, not only its ID.
* Dependencies: only `dependency_type == "required"`. `embedded` is already inside the jar,
  `optional` and `incompatible` we do not touch.
* `skipped[].reason` uses the same values as Modrinth's own resolver (`labrinth/types.ts:41-50`),
  including `quilt_fabric_api` ("Quilt can do Fabric API itself").

Errors: `409 server_busy`, `409 disk_limit_reached`, `422 no_compatible_version`,
`422 unresolvable_dependency`,
`429 upstream_rate_limited`, `502 upstream_unavailable`.

`disk_limit_reached` is asked twice: before resolution, so that an account already over its limit
does not first cost one Modrinth call per project, and afterwards with the **sum of the `size`
values** of the resolved plan — Modrinth states them per file, so nothing gets downloaded only to
notice afterwards that it does not fit.

### 8.8 `POST /api/v1/servers/:id/content/upload`

Permission: `SETUP`. `multipart/form-data`, field name `file`, allowed more than once. `.jar` and
`.zip` are allowed; `.mrpack` belongs in 8.10. Response `200` `ContentUploadResponse`.

The "unknown file" warning (`UnknownFileWarningModal`) happens **before** the upload, in the
browser: take the SHA-1 of the file and ask `GET /v2/version_file/{hash}?algorithm=sha1` through
the pass-through (8.15); 404 means "not on Modrinth". No endpoint of our own needed.

Limit: `max_upload_bytes` — **the same setting and the same default (4 GiB) as in the file
manager**. A panel with two upload limits would be a trap.

Errors: `409 server_busy`, `409 disk_limit_reached`, `413 file_too_large`,
`415 unsupported_file_type`.

`disk_limit_reached` falls as in 7.8 **before the first byte is read**: what `Content-Length`
announces is checked against the owner's free remainder (12.7); without an announcement, only
whether there is any room left in the pot. The disk limit needs the same door here as it does
there, otherwise the very user the file manager turns away tops up here by up to
`max_upload_bytes` per file.

**The `multipart/form-data` reader (8.8 and 8.10, the only two endpoints of this area without
JSON) has three rules you cannot guess** (`content/multipart.rs`):

* It reads the body as it arrives and writes every file part straight into the run's working
  directory. An upload of four gibibytes must not be four gibibytes of panel.
* **A boundary is only a boundary if a line break or `--` follows it.** Without this check, a jar
  that happens to contain the bytes of the boundary cuts its own part in two — and sooner or later
  a jar contains every byte sequence. For the same reason the reader holds back a match it has
  started instead of emitting it already (`content/multipart.rs:220-222`, counter-check
  `:479-480`).
* In a header **only the semicolons outside quotes** separate, otherwise a file name with a
  semicolon would take the line apart; and `name` must not match the end of `filename`
  (`content/multipart.rs:351-353`).

The file name from a browser is also chosen by the attacker and often a whole Windows path: only
the last piece survives, and even that only if it names something. On disk the parts land under a
name of our choosing anyway — **no name from the wire ever reaches the file system**
(`content/multipart.rs:77-78,89-91`).

### 8.9 `POST /api/v1/servers/:id/content/dependents`

Permission: `BASE_READ`. Request `ContentIdsRequest`, response `200` `ContentDependentsResponse`.
The question: if I delete these files — which of the remaining content needs them? Only
`dependency_type` `required` and `embedded` count.

**The call blocks the delete dialog:** `content-tab/layout.vue:374-378` waits for it before any
window appears at all, and swallows every error into `null`. So it has to be answerable fast and
**without a Modrinth call**. That is the reason for writing the dependency list down next to the
file at install time. Empty list → the provider returns `null` and `layout.vue:379` skips the
dialog.

### 8.10 `POST /api/v1/servers/:id/content/modpack/install`

Permission: `SETUP`. The server has to be stopped. Request `ModpackInstallRequest` as JSON (source
`modrinth`) or as `multipart/form-data` with `file` (`.mrpack`) and `meta`
(`{"source":{"kind":"upload"},"keep_extra_content":false}`). Response `202` `OperationAccepted`,
`kind: "install_modpack"`.

`keep_extra_content: false` deletes all content that does not come from the pack beforehand — that
is Modrinth's "Reinstall" behavior; `true` leaves it lying there (the behavior when updating).

Errors: `409 modpack_already_linked`, `409 server_running`, `409 server_busy`,
`409 disk_limit_reached`, `415 unsupported_file_type`, `422 invalid_modpack`,
`502 upstream_unavailable`.

With an uploaded pack, `disk_limit_reached` falls before the first byte is read, as in 8.8;
otherwise the archive would already be in the owner's work area when the refusal arrives. How
**big** the pack unpacks to is in its own `modrinth.index.json` and therefore inside the archive;
so the only thing checked here is the remainder in the pot, not the unpacked total.

### 8.11 `POST /api/v1/servers/:id/content/modpack/update`

Permission: `SETUP`. Request `ModpackUpdateRequest`; `version_id: null` = newest version in the
permitted channel. Downgrading is allowed, the dialog warns already. Response `202`
`OperationAccepted`, again `kind: "install_modpack"` — the same work, the same lock.

Sequence: build the new file list from the `.mrpack`, compare it with the old one, delete removed
ones, replace changed ones, lay out `server-overrides/` and `overrides/` afresh. Content added by
the user stays untouched. That is what the dialog text promises.

Errors: `409 modpack_not_linked`, `409 server_busy`, `409 disk_limit_reached`,
`422 invalid_modpack`, `502 upstream_unavailable`.

### 8.12 `POST /api/v1/servers/:id/content/modpack/unlink`

Permission: `SETUP`. No body. Response `200` `ModpackUnlinkResponse`. The files stay where they
are, but they lose their origin: `source_kind` switches from `modrinth_modpack` to `local`, and
from now on they show up in the main list instead of in the modpack dialog. Errors:
`409 modpack_not_linked`, `409 server_busy`.

### 8.13 `GET /api/v1/servers/:id/content/game-version/preview`

Permission: `BASE_READ`. Query `game_version` (required), `loader`, `loader_version`. Response
`200` `GameVersionPreviewResponse`.

A preview of what becomes incompatible. It maps onto `ContentDiffPreview`
(`installation-settings/types.ts:38-63`) without contortions; `ContentDiffPreview.newLoaderVersion`
is **not** nullable, our `new_loader_version: null` becomes `""` in the provider.
`installation.vue:898` expects `null` when there is nothing to report (empty list **and**
`has_unknown_content == false`) — our response may be both at once, the provider turns it into
`null`.

The preview costs one Modrinth call per affected project and can be canceled
(`installation.vue:890,896` passes a `signal`) — the endpoint has to react cleanly to a client end
that has gone away.

Errors: `400 invalid_request` (unknown `game_version`), `502 upstream_unavailable`.

### 8.14 `POST /api/v1/servers/:id/content/game-version`

Permission: `SETUP`. The server has to be stopped. Request `GameVersionChangeRequest`, response
`202` `OperationAccepted`, `kind: "change_game_version"`.

`incompatible_content` decides what happens to content for which there is no matching build:
`update_then_disable` (update where possible, otherwise disable), `disable`, `keep`.

Errors: `400 invalid_request`, `409 server_running`, `409 server_busy`,
`409 disk_limit_reached`, `502 upstream_unavailable`.

`disk_limit_reached` as in 8.6 and with `0`: with `update_then_disable` the operation downloads a
replacement for every piece of content that no longer fits, and how many those are is something
only the operation itself knows: it asks Modrinth per item.

### 8.15 `GET /api/v1/modrinth/*path`

Permission: signed in. Passes **read** requests through to `https://api.modrinth.com`. The browser
gets `labrinthBaseUrl: '/api/v1/modrinth'`; we change nothing in `@modrinth/api-client` itself.

Only these `GET` patterns are enabled: `/v2/search`, `/v3/search`, `/v2/project/{id}`,
`/v3/project/{id}`, `/v2/projects?ids=[…]`, `/v2/project/{id}/version`, `/v2/version/{id}`,
`/v2/versions?ids=[…]`, `/v2/version_file/{hash}`, `/v2/tag/game_version`, `/v2/tag/loader`,
`/v2/tag/category`, `/v2/user/{id}`, `/v2/team/{id}/members`. Everything else: `403 forbidden`.
No `POST`, `PATCH`, `DELETE`: we are not a Modrinth client with a sign-in.

Status code and body are passed through unchanged, with `Cache-Control` from our cache. On `429`
from Modrinth we answer `429 upstream_rate_limited`, on network errors `502 upstream_unavailable` —
in **our** error format, so the client has to understand one thing and not two.

The `Content-Type` travels back unchanged as well, and that is why
`X-Content-Type-Options: nosniff` goes out with it: Modrinth is a stranger, and an HTML body from
some intermediate server in front of it would otherwise run on **our** origin as soon as someone is
talked into opening the address (`api/content.rs:432-434`, counter-check `:1139-1140`).

Why not straight from the browser, in this order: (1) the rate limit can be steered centrally — one
counter for all windows of all users instead of one per tab, and behind NAT the block otherwise
hits everyone at the same time; (2) the browser does not necessarily have internet, the server does
(it downloads the jars anyway); (3) one cache, two beneficiaries — the version list for the update
dialog is something the backend fetched for the update check regardless; (4) the `User-Agent` can
only be set reliably on the server side.

**The update dialog needs this pass-through**, not only the search: `ContentUpdaterModal`
(`content-updater-modal/index.vue:419`) demands complete `Labrinth.Versions.v2.Version[]` including
`changelog`, `game_versions`, `loaders` and `date_published`, and `currentVersionId` **has to
appear in the fetched list**. Otherwise the "Current" badge stays off, `isDowngrade` is always
`false`, and the update button can be pressed onto the version that is already installed.

### 8.16 Update check

**Who:** the backend and nobody else. The browser never asks Modrinth for updates; it reads
`has_update` and `update_version_id` from 8.1.

| Trigger | Behavior |
|---|---|
| `GET …/content` without `refresh_updates` | cache only; if it is older than 6 h, a check runs in the background and `content_changed` is sent afterwards |
| `GET …/content?refresh_updates=true` | check kicked off at once, the response does not wait |
| after every installation or update | only for the affected projects |
| background run | every 6 h per server, staggered, only for running and recently used servers |

The check uses `GET /v2/project/{id}/version` with `include_changelog=false`; what gets picked is
the newest version that fits the game version and the loader, satisfies the channel rules
(`update-channels.ts:46-77`, `newestEligibleUpdate` — exported, called nowhere, meant exactly for
whoever fills this contract and to be rebuilt in Rust) and is newer than the installed one.

Caching happens in SQLite, in four tables: `modrinth_project_versions` (`project_id` → version
list, `etag`, 6 h, after that with `If-None-Match`), `modrinth_version` (24 h), `modrinth_project`
(`slug`, `title`, `icon_url`, `description`, `project_type`, `downloads`, `followers`, `team`,
`environment`, 24 h) and `modrinth_project_owner` (7 days). The last two are not trimmings:
`project.title`, `project.icon_url`, `owner` and the modpack figures are in **no** version
response. Without them, opening the page costs 41 Modrinth calls for 40 mods.

`modrinth_project` is **filled** in two places, and both talk to Modrinth anyway: the operation
that writes the rows (8.6, 8.7, 8.14 — before the first byte, so that a run broken off halfway
leaves rows that can be named), and the check itself, for every project on the server including the
modpack's. Both fetch `GET /v2/projects?ids=[…]`: **one** call for a whole page, in chunks of 100
IDs, and only for what is missing or older than 24 h. 8.1 never fills — it reads the cache and
nothing else. On top of that, 8.7 fetches the projects of the requested IDs **before** it computes
the plan: the `project_type` from it decides which versions fit at all, because a datapack names
only `datapack` as its loader.

Consequence for rows that are already installed: they get their title at the next check that falls
due — with `refresh_updates=true` (or any installation/update) at once, otherwise at the latest six
hours after the last check, picked up by the background run within 15 minutes. A project Modrinth
no longer knows is missing from the response; the row then keeps its file name, and the check runs
on anyway.

**The check does not lock.** It is not an `Operation`, sets no `busy_reason` and triggers no `409`
— otherwise a delete click would fail at random only because a check happened to be running. The
only thing that makes it visible is `content_changed` with `reason: "updates_checked"`.

**But only ever one runs per server**, and that takes a note of its own inside the process:
`updates_checked_at` is written only at the **end** of a check, so every read of 8.1 during a
running check would see the old timestamp and kick off the next one. Forty mods are forty Modrinth
calls; ten reloads of the page must not be four hundred (`content/mod.rs:76-79`, counter-check
`content/tests.rs:1805-1807`).

After a run **only the projects it touched are checked**. A whole pass would otherwise cost one
call for every mod on the server because a single one was replaced, and `updates_checked_at` would
afterwards claim a check that never took place for the rest (`content/mod.rs:807-810`).

Outbound Modrinth requests run through a shared token bucket (default a conservative 300/min,
`X-Ratelimit-Remaining` is read and the bucket adjusted); on `408`, `429`, `500`, `502`, `503`,
`504` they are retried with a growing wait — the same codes as Modrinth's own client
(`features/retry.ts:85`). A background run over 200 projects is throttled to 60 requests per
minute, so that search and version list keep priority. `User-Agent` fixed to
`<panelname>/<version> (+<repo-url>)`.

### 8.17 The `.mrpack` structure

A ZIP with `modrinth.index.json`; what is evaluated is `name`, `versionId`, `dependencies`
(`minecraft`, `forge`, `neoforge`, `fabric-loader`, `quilt-loader`) and `files` with `path`,
`hashes` (`sha1`, `sha512`), `downloads`, `fileSize` and optionally `env`. Alongside them
`overrides/` (always lay out), `server-overrides/` (lay out on a server, overrides `overrides/`)
and `client-overrides/` (ignore).

Rules that follow: files with `env.server == "unsupported"` are **not** laid out — a server has no
use for a client-only mod. That is why `pack_client_retained` is **always `false`** (the field
means "a client mod that was installed as a dependency"), and `pack_client_depends` is `true` when
a file that was laid out has a required dependency on a project left out this way. Files from
non-Modrinth sources get `external: true` and `external_url`; we download them, check the `sha512`
from the index and reject on a mismatch. That same `sha512` is the basis for "Repair" (9.15).

The structure of `files` and the three override folders are taken from the published format
description and **are to be checked against a real `.mrpack`** before P3 begins — the reference
clone contains only the two frontends, not the Rust side that unpacks a pack. Caught up since:
checked against real packs from Modrinth on 2026-08-12 (`Create Lite 1.1` sits as a test case in
`content/mrpack.rs:320`).

**A pack is a ZIP somebody else built, and two things inside it name paths** — the `path` of every
entry in `modrinth.index.json` and the entry names of the override folders. **Both** go through the
jail from 7.1 before anything is written; the first half is the one no ZIP library checks for us. A
pack that names a single bad path is rejected **as a whole** — a half laid-out pack is worse than
none. An absolute path would land inside the server anyway, but it is a declaration of intent and
is therefore rejected as well (`content/mrpack.rs:1-6,156-163`, counter-checks `:380-381`, `:395`,
`content/tests.rs:963-965`).

Two upper limits keep the bomb out: an entry count above which it is no longer a modpack but an
attack, and a ratio of uncompressed to compressed — a real pack stays far below it, because its
jars are compressed already (`content/mrpack.rs:19-23`, counter-checks `:468-469`, `:478-479`).

`server-overrides/` is laid out **after** `overrides/` and wins because of it, and it is laid out
into the run's working directory, never into the server directory (`content/mrpack.rs:222-224`).

---

## 9. Settings, ports and loaders

### 9.1 `GET /api/v1/servers/:id/properties`

Permission: `BASE_READ`. Response `200` `ServerProperties`. Reads the file fresh from disk on every
call. If it is missing (Velocity, or never started), both objects are empty and the status is `200`
anyway. The page then shows its warning (`properties.vue:5-9,429`).

The 25 keys under `known` are **exactly** the list from `properties.vue:333-359`; it is nailed down
in the vendored code and therefore binding for the backend as well. In the file they carry hyphens,
in the contract underscores (`spawn-protection` → `spawn_protection`); this rewriting applies
**only** to these 25. `custom` keeps the raw spelling, because arbitrary foreign keys live there
(`enable-command-block`, `query.port`).

The rewriting ends with these 25 and happens **nowhere automatically**: `level-name` is not
`level_name`, `enable-command-block` and `query.port` are not ours and keep their spelling. The
other way round, `spawn-protection` in `custom` and `spawn_protection` in `known` are the same line
of the same file — which is why on writing, too, the name decides and not the bucket
(`settings/known.rs:3-7,59-60,83-85`).

**No schema from the server.** The client knows keys and display types statically; a schema
endpoint would be a second truth next to `KNOWN_PROPERTIES`. On the wire all values are therefore
strings — `server.properties` is a Java `.properties` file. Type checking sits with the backend,
because there it holds for calls from outside the interface as well.

**And a Java `.properties` file is older than it looks.** `=`, `:` or bare whitespace separate key
from value, a backslash at the end of a line continues it, and `\t`, `\n`, `\uXXXX` and relatives
are escapes on both sides. So reading works like `java.util.Properties.load`, writing like `store`
— **without** the ISO-8859-1 detour, because Minecraft itself reads and writes the file as UTF-8,
and anything else on disk was left there by a tool that took it for Latin-1. Comments, blank lines
and order survive a write: an entry nobody touched goes back out exactly as it came in. A single
broken escape is taken literally instead of thrown — a settings page that will not open because of
it is worse (`settings/properties.rs:1-9,215-216,230-232`).

**Reading has a lid.** The account that runs the game may write into this file whatever it likes;
pulling it into memory without an upper limit would turn a `GET` into as much memory as it felt
like writing. A real `server.properties` stays under two kilobytes (`settings/disk.rs:42-44`).

### 9.2 `PATCH /api/v1/servers/:id/properties`

Permission: `ADVANCED`. Request `ServerPropertiesPatch` — **changed keys only**; `null` deletes the
line from the file. Response `200` `ServerProperties` with the new state.

A key may sit in both buckets; **the name decides, not the bucket**. Otherwise the contract breaks
as soon as our list and the one in the vendored code drift apart. Lines of the file that are not
named stay **unchanged**: the page sends no full state, only a difference.

Checks in the backend (the client checks nothing): whole number for `max_players`, `max_tick_time`,
`pause_when_empty_seconds`, `player_idle_timeout`, `simulation_distance`, `spawn_protection`,
`view_distance`; `difficulty` ∈ `peaceful|easy|normal|hard`; `gamemode` ∈
`survival|creative|adventure|spectator`; boolean keys only `true`/`false`; no line break and no
null byte in the value; keys as `[A-Za-z0-9._-]+`. The `message` always names the key.

**Two keys belong to the panel:** `server-port` and `query.port` are written by 9.10 →
`409 property_is_panel_owned`. Without this lock the same file would have two writers with no order
between them.

**What a running server does with the file.** Minecraft reads `server.properties` at start and
**writes it anew on a clean shutdown**, from its image in memory. Any change made into a running
instance would be gone again at the next stop. Decision: the backend writes immediately anyway (the
page expects it, `properties.vue:447-449` reloads on a state change), but additionally remembers
the change and **replays it after the stop**, before the process starts again. Without that replay,
"Save & restart" loses exactly the change it was pressed for.

The basic block of the page is **always** rendered, even when a key does not appear in the file at
all (`isPropertyVisible` returns `true` unexamined while no search is active,
`properties.vue:578-581`). So a `PATCH` creates keys that were not there before, among them
`allow_cheats`, which an ordinary `server.properties` does not know. The backend writes them
anyway: an unknown line does not bother the server, and a refusal would be inexplicable to the
user.

Errors: `400 invalid_property_key`, `400 invalid_property_value`,
`409 property_is_panel_owned`, `409 properties_unsupported` (proxy without the file).

### 9.3 `GET /api/v1/servers/:id/startup`

Permission: `BASE_READ`. Response `200` `StartupOptions`.

`java_version`, `jre_vendor`, `startup_command` and `original_invocation` are named exactly as in
`Archon.Content.v1.RuntimeOptions`, so that `advanced.vue:347-352` reads them unchanged. Four
fields are our addition:

* `memory_mib` — the `-Xmx` managed by the panel (`docs/PLAN.md:254-256`).
* `memory_max_mib` — upper bound for the slider: the owner's remaining budget plus what this server
  already holds. If the **owner** has no limit (12.7), it is the machine's
  `assignable_memory_mib` — for an ordinary collaborator on that owner's server as well, because
  what is measured is the owner and not the caller. For a panel admin as the caller, the machine
  too.
* `managed_flags` — the flags the panel sets itself (today `-Xmx`). They stand **next to** the
  command, not inside it.
* `stripped_flags` — **always empty** here. It is the answer to a `PATCH` and lives only in that one
  response (9.4).

`startup_command` is the command as the interface shows and edits it: runtime, `extra_flags`, jar
and loader arguments — **without** `managed_flags`. `original_invocation` is the same command
without `extra_flags`; `advanced.vue:97-111` shows the "Default" button from it. The line that
actually starts is built at start time from the database row and carries `-Xmx` (9.4, "a template,
not a command").

**Why `-Xmx` is not in the field.** The page sends the command back on every save, unchanged ones
included (`Advanced.vue:324`). So everything the panel writes in there comes back to it, and an
`-Xmx` in it came back out as "the panel removed your flag", although nobody had typed one, with
the memory size from some earlier time. The free field carries only what belongs to the user;
`-Xmx` stays out of it (`docs/PLAN.md:416-418`). Anyone who does write one in still gets the
message from 9.4 — and then it means them.

### 9.4 `PATCH /api/v1/servers/:id/startup`

Permission: `ADVANCED`, for `memory_mib` the budget check on top, for `startup_command` **panel
admin** on top. Request `StartupOptionsPatch`, every field omittable; `null` for
`java_version`/`jre_vendor` means "pick automatically again". Response `200` `StartupOptions`
**after** the cleanup.

**The startup command belongs to the panel admin.** Anyone who is not one gets a `403 forbidden`
for a `startup_command` sent along — the owner on their own server too, and even when the field
comes back unchanged: what is refused is holding the field, not the difference inside it. A flag
Java does not know stops the server before the first console line; that is why the interface shows
the field only to a panel admin, and this is the rule behind it. The remaining fields stay at
`ADVANCED`: memory is set by every editor, and the backend checks the runtime choice against what
the machine has anyway (9.5).

**`-Xmx` is cleaned out, not rejected.** The startup command stays a free text field. A `400` on
every `-Xmx` would trigger the only error message the page knows ("Failed to update server
arguments", `advanced.vue:405-408`) — the user would not learn why. Instead the backend removes
`-Xmx`, `-Xms`, `-XX:MaxRAM*` and `-XX:MaxHeapSize` from `extra_flags`, reports them under
`stripped_flags` and sets its own value; `Advanced.vue:322` keeps the response body, the user sees
the result at once. The throttle stays tight (`docs/PLAN.md:342-344`), without the interface having
to fight.

**The message belongs to the save that triggered it.** `stripped_flags` stands in the response to
that `PATCH` and nowhere else, not in the database row, not in the next `GET`. A notice that
survives the save is no longer an explanation but a standing complaint, and its number soon stops
matching the server: measured on 2026-08-15, a server with 4096 MiB carried the sentence "the panel
removed `-Xmx11776M`". To get rid of the sentence you reload the page; nobody has to save for it.
The `stripped_flags` column in `servers` has been neither read nor written since.

**The startup command is a template, not a command.** What is stored is not the string but the
breakdown: Java path (from version and vendor), managed flags, `extra_flags`, jar path, loader
arguments. `argv` is built from it at start time and never from an input
(`docs/PLAN.md:191-192`). Everything the backend does not recognize as a flag in the typed command
(a different jar name, `&&`, pipes) falls away and is likewise reported under `stripped_flags`.

If the chosen runtime is known but not installed (`installed: false`), the backend fetches it on
save; that is an `install_java` operation (5.8), and the response still comes back at once.

`budget_exceeded` and `over_limit` apply only when the **owner** has a memory limit. If they have
none, `memory_mib` is not refused — not even above the machine's size. That is deliberate: 4.2 only
warns on overbooking, and two ways to the same `-Xmx` must not be strict in different measures. The
slider still ends at `memory_max_mib`; above it Java usually starts anyway and the kernel steps in
later (`oom_killed`).

**The arithmetic saturates.** `memory_mib` is a `u32` on both sides, and a panel admin may write a
very large number into a row; the largest number the field accepts, plus what the owner's other
servers hold, fits into no `u32`. Measured before the saturating addition:
`attempt to add with overflow`, and in the release build, where that wraps silently, the same call
answered `200` and wrote an `-Xmx` of four terabytes into the row
(`api/settings.rs:742-744,1892-1896`).

**If the `PATCH` names no runtime, none is checked.** The three refusals about the runtime are
three different questions — a major version that does not exist here, a vendor that does not exist
here, and a pair that exists only as two halves. But changing a flag is not the moment for someone
to find out that there is no Java 11 on this machine (`api/settings.rs:288-291`, counter-check
`:1252-1253`).

Errors: `403 forbidden` (`startup_command` from a non-admin), `400 invalid_java_version`,
`400 invalid_jre_vendor`, `404 runtime_not_installed`, `400 invalid_startup_command`,
`400 memory_too_small`, `409 budget_exceeded`, `409 over_limit`.

### 9.5 `GET /api/v1/java-runtimes`

Permission: signed in, panel-wide. Query `server_id` (optional). Response `200` `JavaRuntimeList`.

`installed: false` means: known and obtainable, but not on disk yet.
`default_major_for_game_version` is only set when `server_id` comes along.

The hard-wired lists in `advanced.vue:285-291` (Java 8/11/17/21/25) and `:341-345`
(`corretto`/`temurin`/`graal`) are filtered against this list, otherwise the interface offers
runtimes the machine does not have. The preselection by game version (`advanced.vue:317-339`) stays
as it is — it agrees with Mojang's `javaVersion.majorVersion` (checked: 1.21.8 → 21).

**`java -version` is not asked.** Every JDK carries a `release` file with `JAVA_VERSION` and
`IMPLEMENTOR`; that costs one read instead of one process per candidate, and the list is needed for
every server row. Two spellings meet on the same machine: `1.8.0_422` is Java 8, `21.0.4` is Java
21 — the leading `1.` fell away after Java 8. And because `JreVendor` knows only three values, and
the page filters against them, a plain OpenJDK build has to answer as one of the three; Temurin is
Eclipse's build of exactly that and therefore the least wrong name
(`settings/runtimes.rs:3-6,179-180,191-193`).

The list is cached: long enough that a page full of servers does not walk `/usr/lib/jvm` for every
row, short enough that a freshly installed JDK shows up without a restart
(`settings/runtimes.rs:17-18`).

### 9.6 `GET /api/v1/servers/:id/allocations`

Permission: `BASE_READ`. Response `200`: **a bare list** `Allocation[]`, ascending by port,
**without** the primary port.

Both are binding, not a matter of taste: `network.vue:275` and `:332` call `.map` and `.find`
directly on the response — an envelope `{allocations, …}` would take the page apart with
`allocations.map is not a function`. And `network.vue:269-281` puts the primary port in front
itself, out of `server.net.port` — if we delivered it too, it would stand in the table twice.

The port pool does not belong here: the page does not show it, and it is in 12.10.

### 9.7 `POST /api/v1/servers/:id/allocations`

Permission: `ADVANCED`; only a panel admin may set `port` (`docs/PLAN.md:350-353`), otherwise the
panel hands out the next free port from the pool. Request `CreateAllocationRequest`, response
`201` `Allocation`.

Search and entry run in **one** transaction with a unique constraint on the port. Otherwise two
concurrent calls reach for the same number and the second one fails only when the server starts.

Three kinds of collision are deliberately kept apart: `409 port_in_use` (belongs to a server of
this panel, fixable by releasing it), `409 port_unavailable` (a foreign process holds it; checked
with a short bind attempt at creation time), `403 port_out_of_pool` (a permission question). Plus
`409 port_pool_exhausted`, `400 invalid_port` (not 1024–65535), `400 invalid_name` (empty or longer
than 32 characters), `409 allocation_limit` (more than 8 per server).

### 9.8 `PATCH /api/v1/servers/:id/allocations/:port`

Permission: `ADVANCED`. Only the name can be changed: the port is the key. Request
`RenameAllocationRequest`, response `200` `Allocation`. Errors: `404 allocation_not_found`,
`400 invalid_name`.

### 9.9 `DELETE /api/v1/servers/:id/allocations/:port`

Permission: `ADVANCED`. Response `204`; the port goes back into the pool. Errors:
`404 allocation_not_found`, `409 primary_allocation` (the primary port cannot be deleted, only
swapped).

Modrinth's warning text "This cannot be reserved again" (`network.vue:36`) is wrong for us and
belongs changed in our version.

### 9.10 `PUT /api/v1/servers/:id/allocations/:port/primary`

Permission: `ADVANCED`. No body. Response `200` `SetPrimaryResponse`.

The previous primary port stays with the server as an ordinary allocation. Otherwise a swap would
lose it to the pool without a word. The backend writes `server-port` and `query.port` into
`server.properties`; while the server runs this only takes effect after the restart, hence
`restart_required: true`. The same replay as in 9.2 applies.

This endpoint does not exist in the contract code; it is served by the row we add in `network.vue`
and therefore does not run through the client adapter. Errors: `404 allocation_not_found`,
`409 already_primary`, `409 playit_tunnel_exists`.

`409 playit_tunnel_exists` means: this server has a public address through playit.gg (18.8). Where
a tunnel points is held at playit and cannot be changed from here; a swap would leave the hole from
the internet standing on a number this server no longer holds, and the pool gives a freed port to
the next server. Hand the address back first, then swap. The account it hangs on can hand it back —
the owner's (18.9).

### 9.11 `GET /api/v1/loaders`

Permission: signed in. Response `200` `LoaderList` with exactly ten entries.

A **unified** catalog, no foreign formats passed through. Five reasons: the backend needs the same
data anyway to fetch the file and check it; the formats differ irreconcilably (Paper v3 a bare list
with `downloads["server:default"].checksums.sha256`, Purpur `{builds:{all:[…]}}` with `md5`, Leaf
the old Paper v2 format with `downloads.primary`, Fabric three separate lists, Mojang a manifest
with a second round per version); the second wave partly delivers no JSON at all; PaperMC asks for
a `User-Agent` that says who it is, which a browser cannot set; and a cache in the backend keeps
the foreign APIs out of it.

`availablePlatforms` of the installation page is `loaders.filter(l => l.wave <= currentWave)
.map(l => l.id)`. `supports_properties: false` on Velocity is the reason the "Properties" tab stays
empty.

**The IDs are lowercase** (`vanilla`, `paper`, `folia`, `purpur`, `leaf`, `fabric`, `velocity`,
`neoforge`, `quilt`, `forge`), because the layout expects `toLowerCase()` everywhere
(`installation.vue:268,477`). The **display name** is in `name` — the adapter puts that into the
vendor server object instead of using `formatLoaderLabel`, which would turn `neoforge` into
"Neoforge".

**The creation wizard** gets `availableLoaders` as a plain `string[]` out of `loaders[].id`
(`components/flows/creation-flow-modal/index.vue:29`); `formatLoaderLabel` labels unknown things by
capitalizing the first letter, and that carries for all ten there.

### 9.12 `GET /api/v1/loaders/:loader/game-versions`

Permission: signed in. Response `200` `GameVersionList`, **newest first**. Serves
`resolveGameVersions` and `resolveHasSnapshots`.

`version_type` is `release` or `snapshot`; for Vanilla and Fabric the distinction comes from the
source (Mojang `type`, Fabric `stable`), for Paper/Folia/Purpur/Leaf/Velocity from the spelling
(`-rc`, `-pre`, `-snapshot`, `-SNAPSHOT`).

For the installation page this endpoint replaces Modrinth's `injectTags().gameVersions`
(`installation.vue:486-518`), which pulls the complete list from `api.modrinth.com` and then cuts
it against the loader's list. We cut in the backend, where the loader list already lies.

**Velocity has no game versions.** There we put the Velocity series into this axis ourselves
(`3.5.1`, `3.4.0-SNAPSHOT`, …), because the form does not become valid without a selected "game
version" (`use-installation-form.ts:62`). The build number stays the second axis — the same split
in two as with Paper, without special handling in the layout.

Errors: `404 loader_not_found`, `502 upstream_unavailable` (`message` names the source).

### 9.13 `GET /api/v1/loaders/:loader/game-versions/:game_version/builds`

Permission: signed in. Response `200` `LoaderBuildList`, **newest build first**. Serves
`resolveLoaderVersions`.

The order is part of the contract: `loaderVersionEntries[selectedLoaderVersion]` is addressed by
**index** (`use-installation-form.ts:214-216`), and `selectedLoaderVersion` is set to `0` on every
change. That brings a quirk you have to know: `handleStartEditing` does **not** look for the
installed build, so it sits on index 0 = newest build, and `hasChanges` reports a change
immediately as soon as the server is not running on the newest build. **Our build list must
therefore always contain the installed build**, even when the foreign source cleared it away long
ago — only `cancelEditing` looks for it and otherwise falls back to 0.

`channel_tag` knows only `"ALPHA"`, `"BETA"` or `null`, because `PaperChannelBadge.vue:23` has
only these values. The contract type `LoaderVersionEntry` writes `channelTag` in camelCase and
knows no `null` — the renaming and the dropping of the field on `null` is done by our side, one
line.

`released` is **nullable**: Paper, Folia, Velocity and Leaf deliver a timestamp per build, Purpur's
build list is `builds.all: string[]` (numbers only, a date would cost one call per build) and
Fabric's `/v2/versions/loader/{game}` has none at all. We do not fetch it afterwards.

Hard upper limit **500 builds**, then `truncated: true` — measured: Leaf alone has 168 builds for
`1.21.8`, old Paper series run to four digits. For `vanilla`, `builds` is empty; the layout does
not ask there anyway.

Errors: `404 loader_not_found`, `404 build_not_found` / `422 unsupported_game_version`,
`502 upstream_unavailable`.

**The sources, checked against the real APIs on 2026-08-12:**

| Loader | Version list | Build list | File | Checksum |
|---|---|---|---|---|
| Vanilla | `launchermeta.mojang.com/mc/game/version_manifest_v2.json` | — | second call on `versions[].url` → `downloads.server.url` | `downloads.server.sha1` |
| Paper | `fill.papermc.io/v3/projects/paper` (groups → list) | `/v3/projects/paper/versions/{v}/builds`, bare list | `downloads["server:default"].url` | `…checksums.sha256` |
| Folia | like Paper, project `folia` (from 1.19.4) | like Paper | like Paper | like Paper |
| Velocity | like Paper, project `velocity`; the keys are Velocity series | like Paper | like Paper | like Paper |
| Purpur | `api.purpurmc.org/v2/purpur` (ascending!) | `/v2/purpur/{v}` → `builds.all` as strings | `/v2/purpur/{v}/{build}/download` | `md5`, **no** sha256 |
| Leaf | `api.leafmc.one/v2/projects/leaf` | `/v2/projects/leaf/versions/{v}/builds` (ascending) | `…/builds/{b}/downloads/{name}` | `downloads.primary.sha256` |
| Fabric | `meta.fabricmc.net/v2/versions/game` | `/v2/versions/loader/{game}` | `/v2/versions/loader/{game}/{loader}/{installer}/server/jar` | **none** |
| NeoForge | `maven.neoforged.net/api/maven/versions/releases/net%2Fneoforged%2Fneoforge` | the same list | `…/neoforge-{v}-installer.jar` | — |
| Quilt | `meta.quiltmc.org/v3/versions/loader` | the same list | `…/installer` | `hashes.sha256` |
| Forge | `files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json` | `promos` | installer jar | — |

Three deviations from the plan that came to light: **Leaf's channels are called `default` and
`experimental`**, not `stable`/`experimental` (counted in `1.21.8`: 99 × `default`, 69 ×
`experimental`); mapping `default` → `stable: true, channel_tag: null`, `experimental` →
`stable: false, channel_tag: "ALPHA"`. **Leaf hangs the file under `downloads.primary`**, not under
`downloads.application` like the old Paper v2. **Fabric publishes no checksum** for the server jar
— the file is generated on every request; there we check only TLS and size and write that into the
loader definition instead of pretending we had a hash. The backend picks the Fabric installer
version itself (newest `stable: true`); it does **not** appear in the interface, or the version
selection would have a third axis.

Cache: version and build lists 30 minutes (agrees with Paper's `s-maxage=1800`), the Mojang
manifest 10 minutes, downloaded files permanently under their hash. `cached_until` is in every
response, so the interface does not have to guess.

**Both axes come out of a URL path and get pasted into the address of a foreign service**, and
that is why every version and build name is first checked against the characters the seven sources
actually use. Measured without this check: a `game_version` of `../../velocity/versions/3.5.1`
answered `200` with Velocity's build list under `loader: "paper"`, because the URL parser folds the
`..` away before the request goes out. That is two things at once: a caller who picks which
endpoint of the source the panel asks, and one cache key per spelling
(`settings/catalog.rs:391-398`, counter-check `api/settings.rs:2099-2103`).

**With Purpur and Leaf the spelling is not a second opinion but the only one.** Both hand out every
series as stable; without the rule from the spelling, `1.21.9-rc1` would go out as `release`, and
`resolveHasSnapshots` would see not a single snapshot (`settings/catalog.rs:367-371`, counter-check
`:646-648`).

**The upper limit is the harder of the two promises in 9.13** — harder than "the installed build
has to be in there". Measured before the assembly counted along: five hundred from the source plus
one build a server here runs on answered five hundred entries with `truncated: false`, although one
of the five hundred had to give way for it; six hundred forgotten builds answered six hundred
entries. What gives way are the oldest of the ones shown anyway — `cancelEditing` has to find the
build the server stands on, and nobody else (`settings/catalog.rs:425-443,620-624`).

### 9.14 `POST /api/v1/servers/:id/install`

Permission: `SETUP`. Serves `save` and `saveWithoutAutoFix` of the installation page. Request
`InstallRequest`, response `202` `InstallAccepted` — `OperationAccepted` with
`kind: "install_loader"`, plus optional `warnings`.

`content_policy` is `keep` (world, mods and configuration stay where they are) or `wipe_mods`
(`mods/` and `plugins/` are moved aside). Four families: `vanilla` · Bukkit descendants (`paper`,
`folia`, `purpur`, `leaf`) · mod loaders (`fabric`, `quilt`, `neoforge`, `forge`) · proxy
(`velocity`). Within one family content stays; across family boundaries the backend demands
`wipe_mods`, otherwise `409 loader_change_needs_wipe`. The world survives in both cases. On a
change to or from `velocity` there is also `warnings: ["properties_will_be_ignored"]`, because a
proxy reads no `server.properties`.

`loader_change_needs_wipe` is the server-side counterpart to Modrinth's warning dialog. The warning
in the client is optional and hangs on the content area; the rule in the backend is not.

Because the call comes back at once, the other direction has to be tight: while the operation runs,
4.6 refuses with `409 server_busy`. Without that, a user starts the server into the middle of a
half-written installation.

Errors: `422 unknown_loader`, `422 unsupported_game_version`, `404 build_not_found`,
`409 server_running`, `409 server_busy`, `409 loader_change_needs_wipe`,
`502 upstream_unavailable`, `507 no_space`.

**The two build checks of this endpoint stand where they stand because of two measurements.**

* `404 build_not_found` is put against the **whole** cached list, not against the five hundred from
  9.13. 9.13 caps a selection list; 9.14 does not install from a selection list. Measured
  beforehand: `POST …/install` with build `7` of a series with six hundred builds answered
  `404 build_not_found`, although the installer would have fetched it without complaint
  (`settings/catalog.rs:264-268`, counter-check `:692-695`).
* If the body names **no** build, that means "the newest stable one", and the endpoint still asks
  whether one exists — even though the run resolves it once more right after. Measured on the
  running panel: `POST …/install` with Paper 1.21.5 and without a build answered `202`, the run
  died at three percent on `invalid_version`, and the server — untouched on disk — stayed `broken`
  and could not be started any more, because a failed run marks it that way. So the difference is
  `404 build_not_found` against a `202` that renders a working server useless
  (`settings/catalog.rs:281-285,710-713`, `api/settings.rs:660-664`).

### 9.15 `POST /api/v1/servers/:id/repair`

Permission: `SETUP`. No body. Response `202` `OperationAccepted` with `kind: "repair_content"`.

Downloads the same loader file again, checks the checksum, writes `eula.txt` and the managed parts
of the configuration anew. Does **not** touch world, mods or `server.properties`. With a linked
modpack, checking every file from the `.mrpack` comes on top (SHA-512 from the index against the
file on disk) and pulling missing or changed ones again. Without a linked modpack, only the loader
is reinstalled.

A second path `…/content/modpack/repair` does **not** exist: the button sits in the settings area
(`installation-settings/layout.vue:1070`) and serves `InstallationSettingsContext.repair`. Errors:
`409 server_running`, `409 server_busy`, `502 upstream_unavailable`.

### 9.16 `POST /api/v1/servers/:id/reset`

Permission: `RESET_SERVER` — kept apart from `SETUP`, because it destroys data. Request
`ResetRequest`, response `202` `OperationAccepted` with `kind: "reset_server"`.

Deletes **the entire server folder** — world, mods, configuration, logs — and installs again.
Backups stay; the interface promises that in so many words (`installation.vue:127-130`), so
`keep_backups` is fixed at `true` and a `false` is refused with `400 invalid_request`. The field is
in the contract anyway, so that nobody assumes it was forgotten.

Before the deletion stands the same `chown-tree` as in 4.5, for the same reason: what the game
created has to be opened up to the panel first, otherwise the run stops in a directory that belongs
to the game alone.

Errors: `409 server_running`, `409 server_busy`, `422 unknown_loader`.

### 9.17 `POST /api/v1/servers/:id/reset-to-setup`

Permission: `RESET_SERVER` **and** panel role `admin` — Modrinth's page shows the button only to
panel admins (`installation.vue:266`). No body, response `200` `ResetToSetupResponse`.

Puts the server back into first-time setup: `flows.intro = true`, clear the console buffer, discard
the loader details. **The files stay where they are** — this path is there to redo a setup that
went wrong, not to clean up. Errors: `409 server_running`.

**Our interface no longer calls it.** The "Back to setup" button stood in the danger zone under
"General"; the operator wanted it gone, and it is gone. The endpoint stays, because otherwise a
setup that went wrong could only be rescued through the database, but today it has no caller,
neither in `web/src` nor in the foreign code.

### 9.18 The client adapter

Four of the five settings pages do **not** talk through a `provide` contract but through
`injectModrinthClient()` (`properties.vue:312`, `advanced.vue:233`, `network.vue:242`,
`general.vue:165`). For them there is a subclass of `AbstractModrinthClient` of our own that maps
eleven Archon paths actually in use onto ten targets of our own. This does **not** rebuild Archon's
API; the rewriting is a table in the frontend.

The hook goes on `request()` (`api-client/src/core/abstract-client.ts:122`, public and not
abstract), **not** on `buildUrl()`: that sees neither method nor body nor `params`, and two of the
necessary transformations — name from the query into a JSON body, `PUT` → `PATCH` — cannot be done
there.

| Archon path | our path |
|---|---|
| `GET /modrinth/v0/servers/{id}` | `GET /api/v1/servers/{id}` |
| `GET /v1/servers/{id}` | ditto — the same response; nobody reads `ServerFull`, but the call has to **succeed** |
| `GET|PATCH /v1/servers/{id}/worlds/default/properties` | `GET|PATCH /api/v1/servers/{id}/properties` |
| `GET|PATCH /v1/servers/{id}/worlds/default/options/startup` | `GET|PATCH /api/v1/servers/{id}/startup` |
| `GET /modrinth/v0/servers/{id}/allocations` | `GET /api/v1/servers/{id}/allocations` |
| `POST /modrinth/v0/servers/{id}/allocations?name=X` | `POST /api/v1/servers/{id}/allocations`, `{"name":"X"}` |
| `PUT /modrinth/v0/servers/{id}/allocations/{port}?name=X` | `PATCH /api/v1/servers/{id}/allocations/{port}`, `{"name":"X"}` |
| `DELETE /modrinth/v0/servers/{id}/allocations/{port}` | `DELETE /api/v1/servers/{id}/allocations/{port}` |
| `POST /modrinth/v0/servers/{id}/power` | `POST /api/v1/servers/{id}/power` |
| `…backups_queue_v1.*` and `backups_v1.rename/delete` | 10.1–10.7 and 5.4/5.5 |

The two server fetches are listed here even though the endpoint belongs to the server area: the
host of the tabs fetches **both** before any tab renders, and waits in a `Promise.all`
(`ServerSettingsModal.vue:177,186,191`). A path that is not mapped lands in the `catch` there —
then the whole window stays empty and reports "Failed to load server".

**How the adapter gets in.** Replacing a module on the real client fails: `abstract-client.ts`
creates every module as a getter with `configurable: false` and the namespaces as
`writable: false, configurable: false` — assignment, `defineProperty` and even a `Proxy` throw (on
non-writable, non-configurable fields the proxy invariant forces the same value). What carries is a
foreground over the real client:

```ts
const shim = Object.create(realClient)
Object.defineProperty(shim, 'archon', {
  value: { ...realClient.archon, backups_queue_v1: backupAdapter, backups_v1: legacyAdapter },
  enumerable: true,
})
provideModrinthClient(shim)
```

The spread evaluates the getters and freezes the remaining Archon modules as ordinary fields —
including `sockets` and `sync`, which the concrete client attaches later. `labrinth` and `kyros`
the foreground inherits through the prototype chain. These four lines cross areas: every further
area that replaces a `client.*` module hooks in here.

Two behavioral rules that follow from the code and are easy to miss: errors have to be `Error`
objects whose `message` contains the string `429` on a 429 (`BackupCreateModal.vue:164`,
`use-inline-backup.ts:136` check `error.message.includes('429')`), and `backups_queue_v1.list` has
to deliver `history` in **descending** order.

---

## 10. Backups

This area has **no** `provide` contract. It is served through the client module
`archon.backups_queue_v1` (9.18), which is called from seven adopted files — among them
`composables/server-backups-queue.ts:24`, `use-inline-backup.ts:130,154,163` and
`ServerPanelAdmonitions.vue:295-344`. `InlineBackupCreator` sits in **ten** dialogs spread across
content, files, settings and the creation wizard; backups are therefore not a page of their own
but a cross-cutting dependency.

**The whole server directory is backed up**, not just the world (`docs/PLAN.md:452` says
"world", which is not enough): Modrinth's own texts speak of "world data **and server
configuration**" and "will replace the current world **and server files**"; "Reset" promises
"Backups will remain and can be restored", and a world-only backup would afterwards leave a world
without the mods that produced it; `InlineBackupCreator` is offered before mod deletions and
modpack switches, that is, before operations that touch `mods/` and `config/`, not the world; and
Velocity has no world at all. Excluded: `logs/`, `crash-reports/`, `cache/`, `*.log.gz`, Unix
sockets and everything that points outside (symlinks are not followed). `libraries/` and the
loader jars stay **in** — without them a Forge server will not start after a restore. Format: one
`tar`, zstd-compressed, level 3; no incremental backups, no deduplication — every backup is one
state, and you can also unpack it by hand with `tar`.

**Five rules of the archive itself, each written out of a bug** (`backups/archive.rs`):

1. **The body of every entry is forced to the length in the header.** `tar` writes the size it
   read from the file handle, then copies what the file happens to hold, and pads from the number
   it copied. A file that has grown or shrunk between those two moments shifts every entry behind
   it by a few bytes, and from there on the archive is **silently** unreadable — noticed months
   later, by whoever restores it. Backing up while running is allowed (10.2), and `save-off` only
   stops the world, not the plugin that keeps a file of its own. So: length into the header, and
   `Exactly` holds the body to it — too short is padded with zeros, too long is cut off
   (`backups/archive.rs:241-250`, counter-test `:795-799`).
2. **Hard links are rejected without exception.** `tar` resolves a hard link against the working
   directory, not against the root, and the inode it lands on keeps the permissions of the file it
   shares — which the `chown_tree` after a restore would then overwrite with the game account. We
   never write one ourselves, so nothing is lost (`backups/archive.rs:314-318`).
3. **A symbolic link is judged by *where it lands*, not by how it is written.** Counting the `..`
   against the depth of the link lets `door/..` through as soon as `door` is a link to the root:
   the arithmetic comes out at zero, and the step still leads outside. Ordinary entries are judged
   by how they are written, links by where they land — and because every link the tree can hold is
   thereby demonstrably kept inside, the spelling is enough for the ordinary ones
   (`backups/archive.rs:10-14,451-453`, counter-tests `:685-687`, `:737-739`, `:851-854`).
4. **The target of a link is rewritten relative to its own directory**, and that is not cosmetic: a
   restore unpacks into `<server-dir>.restoring-<run-id>`, and from there an absolute target would
   name the *old* directory — on reading it back it would be rejected, so what we packed could not
   be unpacked (`backups/archive.rs:394-402`).
5. **Missing directory entries are created on unpacking.** A hand-written `tar` may leave them out;
   `tar -x` creates them, and so do we, otherwise a restore stops at the first file in `world/`
   (`backups/archive.rs:332-333`).

**The tree is handed back to the game account before it is *read*** — the same `chown-tree` rule as
after writing (`docs/PLAN.md:205`), only justified the other way round here: Java creates a
temporary file with `0600`, whatever umask the supervisor has set. `level.dat` is written exactly
that way on every save, and the profiler Paper ships with drops a fresh one into
`plugins/spark/tmp-client` every few seconds. Unread, that would be a world that does not come
back. Whatever is still unreadable afterwards came into being between taking stock and packing and
is skipped rather than costing the whole backup
(`backups/mod.rs:834-841`, `backups/archive.rs:220-226`).

**Storage location outside the server directory:** `/var/lib/<panel>/backups/<server-id>/`, owner
`craftpanel:craftpanel`, `0700`. Three reasons: a backup of the whole directory would otherwise
pack all the older ones with it; the file manager shows everything below the server directory, so
an editor could delete backups without the `BACKUPS` bit; and a reset deletes the server directory.
`0700` for `craftpanel` is stricter than `docs/PLAN.md:150-158`, and deliberately so: the server
process runs as `craft-<id>` and would otherwise reach everything that belongs to that user.
Backups therefore do **not** count towards the server's `storage_usage_bytes`.

### 10.1 `GET /api/v1/servers/:id/backups`

Permission: `BASE_READ`. No parameters, no pagination — but only because two upper limits keep the
response small and are therefore fixed: **`max_backups` ≤ 50** (the admin may set the value, not
beyond that) and **`history` ≤ 20 operations per backup**, with the oldest dropping out. In the
worst case that is around 300 KB of JSON. Response `200` `BackupListResponse`.

Order: `backups` descending by `created_at`, `history` descending by `scheduled_for`. The order of
`history` is **mandatory, not cosmetic**: `history[0]` counts as the "last operation"
(`ServerPanelAdmonitions.vue:138`).

`active_operations` is the subset with `state ∈ {pending, ongoing}`, kept flat next to the list.
The redundancy with `history` is intended and cheap; but the subset has to be exactly this one,
otherwise `ServerPanelAdmonitions` shows a permanent banner (entries from `active_operations` are
never dismissible, `:250`).

Three subtleties you cannot guess:

* **`status` has to flip on a restore too.** `hasRunningRestore`
  (`server-backups-queue.ts:63-69`) checks the **cross product** of the operation and
  `backupById.get(o.backup_id)?.status === 'in_progress'`. The restored backup is `done` before the
  operation; if we derived `status` only from the `create` operation, it would stay that way,
  `hasRunningRestore` would always be false, and with it every lock would fall. So: `status =
  in_progress` as soon as an operation of **any** kind is running on this backup, then back to the
  result of the most recent one. Visible price: during a restore the source backup drops out of
  `completedBackups` and disappears from the list for that time. That is Modrinth's behavior.
* **`has_parent`** is `true` **only** for the `create` operation of a safety backup that came out of
  a `restore`. `hasActiveCreate` excludes such operations (`server-backups-queue.ts:49-51`); if we
  set this wrong, the interface blocks during every restore with "A backup is already queued or in
  progress".
* **`should_prompt`** is `dismissed_at === null` of the associated operation (5.5). `true` for every
  end state of a manually triggered operation; `false` for successful **automatic** backups —
  otherwise the panel greets you every morning with a success banner; `true` for **failed**
  automatic ones, because the fact that the schedule is not running is something you have to learn.

Three fields belong to section 22 and appear in **every** backup, including a local one:
`location` (`"local"` or `"drive"`), `drive_state` and `drive_web_link`. For a local backup the
last two are `null`. `location` is the place where the bytes **lie**, not the target that is set
for the next run (22.9): a row keeps its value forever, and changing the target setting moves no
byte.

`locked` is always `false` (we know no locked backups), `synthetic_legacy` always `false` (we have
no legacy baggage), `size_bytes` is `0` as long as `status != "done"`. `size_bytes` stays set for
`location: "drive"` as well — you want to see the size — but it does not count against the
account's disk limit (22.18).
`operation_id` is the ULID of the operation from section 5; the adapter passes it through with
exactly one documented `as unknown as number` line, because `BackupQueueOperation.operation_id` is
declared as `number | null`. Verified: the value is nowhere used for arithmetic, sorting or a `<`
comparison — it is checked for `!= null`, compared for equality, put into a key and returned to the
client.

`user_info` is a `UserRef`. We never send `id: "support"` — the "support staff" role is out
(`docs/PLAN.md:94`), and `BackupItem.vue:79` would show an Intercom icon for it.

### 10.2 `POST /api/v1/servers/:id/backups`

Permission: `BACKUPS`. Request `CreateBackupRequest`, response `202` with the full `Backup`
(`status: "pending"`, one `create` operation in `history`). The body **must** contain `id` —
`use-inline-backup.ts:130` destructures it.

`name`: 1–128 characters after `trim`. The input fields cap at 48, but the name of the safety
backup is cut to 92 characters (`BackupRestoreModal.vue:78-81`) and created over the same path —
**a limit of 48 in the backend would break every restore with a long backup name**. The backend
allows duplicate names; the interface prevents them for manual creation and renaming itself
(`nameExists` disables the button), and the two paths that run past it — safety backup and schedule
— are allowed to produce them.

**Creating while running is allowed.** `backupCreationDisabled` (`backups.vue:618-636`) checks
permission, quota and lock reasons — **not** `isServerRunning`. If the server is running and is not
a proxy, the backend proceeds like this: `save-off`, `save-all flush`, wait for the console line
`Saved the game` (or `Saved the world`), at most 30 s, pack, and **always** `save-on` — on abort,
on error and on timeout too. This is the only path on which this feature can do real damage, and it
belongs in a `Drop` guard, not in an `if` at the end. If the server does not answer within 30 s, we
pack anyway and write a warning to the console; there is no field of its own for that, because the
interface would have nowhere to show it.

Whether Modrinth sends `save-off` is stated in **no** line of the code at hand (search for
`save-off`, `save-all`, `save-on` in `vendor/` and in the reference clone: zero hits). That is our
decision, not theirs.

**Check and insert are one write operation**: `server_busy`, `backup_limit_reached` and
`rate_limited` all three check a state that the same call changes right afterwards. Two concurrent
requests — two windows, or two of the ten dialogs — would otherwise both get through. So: a SQLite
transaction around check and `INSERT`, and a unique partial index "one open backup operation per
server" as a second seam. The same holds for 10.6 and 10.7.

Errors: `400 invalid_name`, `409 server_busy` (a `create` or `restore` is already running),
`409 backup_limit_reached`, `409 disk_limit_reached`, `429 rate_limited` (more often than once per
60 s, `Retry-After`), `507 no_space` (free space < estimated size × 1.1; the estimate is the sum of
the file sizes minus the exclusion list, uncompressed — deliberately pitched too high).

On top of that, as soon as this server's target is `drive` (22.9): `409 drive_not_connected` if the
owner has no Drive connected, and `409 drive_not_configured` if the operator has set nothing up.
**Both space questions still stand** — the archive is built here first in every case (22.15), and
`message` says exactly that, so that nobody mistakes `507 no_space` on a backup "to the cloud" for
a bug.

Two different questions, one after the other: `507 no_space` is the machine, `409 disk_limit_reached`
is the owner's pool (12.7). A machine with a free terabyte still says no when the account is full.
On a scheduled run (10.9) the operation then ends with `error.code = "disk_limit_reached"` and is
not retried.

### 10.3 `PATCH /api/v1/servers/:id/backups/:backup_id`

Permission: `BACKUPS`. Request `RenameBackupRequest`, response `200` with the full `Backup`.
Errors: `400 invalid_name`, `409 server_busy` (an operation on this backup).

### 10.4 `DELETE /api/v1/servers/:id/backups/:backup_id`

Permission: `BACKUPS`. Response `204`. Deletes file and row immediately, no trash: the dialog says
"Deletion is permanent". Errors: `409 server_busy`; running operations are aborted over 5.4, not
over the delete, and the interface already keeps the two paths apart itself.

**Row first, file second.** A row without a file is a backup that cannot be restored; a file
without a row is a few megabytes nobody looks after. For a backup in the Drive the same order
applies (22.18), and there the file may in fact be left behind: it lies in the storage of its
owner, where he sees it and can throw it away, and keeping a row that nobody ever cleans up is the
worse of the two (`backups/mod.rs:500-505`).

### 10.5 `POST /api/v1/servers/:id/backups/bulk-delete`

Permission: `BACKUPS`. Request `BulkDeleteBackupsRequest` (field name from
`DeleteManyBackupRequest`), response `200` `BulkDeleteBackupsResponse`. Partial success is
expressly allowed: deleting files one by one can fail one by one. The interface does not read the
body and reloads afterwards, so whatever is left over becomes visible. `400 invalid_request` if
the list is empty or has more than 100 entries.

### 10.6 `POST /api/v1/servers/:id/backups/:backup_id/restore`

Permission: `BACKUPS`. **The server has to be stopped.** Request `RestoreBackupRequest` — `name` is
mandatory and names the **safety backup** that is created beforehand. Response `202`
`RestoreBackupResponse`.

The safety backup gets `automated: false`, its `create` operation gets `has_parent: true` and
`should_prompt: false`. Otherwise two success banners would stand side by side after every
restore. The restore banner reports on the outcome. So that `has_parent` has something to point at,
the **restore operation is created first** and the copy after it; without this pointer the
interface reads the copy as "a backup is already queued" and locks for the whole duration of the
restore (10.1). If something fails after that, the already open restore operation has to be cleared
away with it. Otherwise it would hold the server against every write until somebody aborts it by
hand (`backups/mod.rs:580-582,589-590`).

Sequence: create the safety backup → unpack only once it has succeeded. If it fails, **nothing** is
restored; the `restore` operation goes to `failed` with `error.code = "safety_backup_failed"`. The
unpacking itself runs into a new directory `<server-dir>.restoring-<operation-id>`, then
`<server-dir>` → `<server-dir>.old-<operation-id>`, then `.restoring-…` into its place, then delete
`.old-…`. If it breaks off before the last step, the rename is undone: the server is never half
restored.

**Both working names carry the operation ID, and that is not cosmetic.** A fixed `.old` stays
behind if it cannot be deleted — the game process may create folders the panel cannot enter
(`docs/PLAN.md:196-205`), and then `rename(<server-dir>, <server-dir>.old)` fails for **every**
further restore of this server with `ENOTEMPTY`, finally and wordlessly. A name that no other run
has cannot stand in the way of the next one. Every run also cleans up what earlier ones left lying
next to the server directory: delete first, on refusal one `chown-tree` and delete again. Whatever
is still standing after that is **written to the server's console** — it lies in
`users/<owner>/servers` and counts against the owner's disk pool (12.7), and a silent `.ok()` would
be exactly the kind of lie nobody notices. If the server directory is missing while an `.old-…`
lies next to it, **nothing** is cleaned up: then that directory is the server and not a leftover.

Errors: `409 server_running`, `409 backup_not_restorable` (`status != "done"`, and since 22.17 also
`drive_state ∈ {missing, trashed}` — with the reason in plain words), `409 server_busy`,
`409 backup_limit_reached` (the safety backup no longer fits in the quota),
`409 disk_limit_reached` (the safety backup no longer fits in the owner's disk pool, 12.7),
`507 no_space`.

The safety backup is created on the server's **configured target**, not at the location of the
restored backup: otherwise the path that fetches a Drive backup back would be exactly the path that
fills the local disk (`backups/mod.rs:365-368`).

The unpacking asks the two space questions from 10.2 as well, and the estimate for it is the
archive **plus four times its own size** for what it unpacks: zstd manages more than that on a
world, and a refusal has to be safe rather than clever (`backups/mod.rs:1156-1160`).

### 10.7 `POST /api/v1/servers/:id/backups/:backup_id/retry`

Permission: `BACKUPS`. No body, response `202` `RetryBackupResponse`. Creates a **new** operation of
the same kind as the most recent failed one. For `create`, the broken file is cleared away first
and the same backup row is reused: the ID stays, so the banner does not jump.

This endpoint exists next to the generic 5.6 because `backups_queue_v1.retry(serverId, worldId,
backupId)` passes the **backup ID**, not the operation ID; 5.6 therefore excludes `backup_*`.

**For `restore` there is no name for a second safety backup** — the call has no body and the
interface asks for nothing. Hence: the retry keeps using the safety backup of the first attempt, as
long as that one is `done`. If it is not `done`, the retry is a full restore with a new safety
backup named `Before restoring "<name>"`, cut to 92 characters. Without this rule you would either
have a restore without a net, or another copy in the quota on every click.

The question asked is for the most recent safety backup **of the backup**, not for the one of the
most recently failed run. On the second retry the difference shows up: the run that failed last
*used* the copy of the first attempt and therefore has none of its own — asking it would answer
"none" and buy a fresh one on every further click. A copy that somebody has deleted in the meantime
leaves `target_id` at `NULL` and reads as "none", which is exactly right here
(`backups/store.rs:240-246`, counter-test `backups/tests.rs:720-722`).

Errors: `409 nothing_to_retry` (`history[0].state` is neither `failed` nor `timed_out`),
`409 server_running` (on `restore`), `409 server_busy`, `409 disk_limit_reached`,
`507 no_space`.

This call asks the two space questions from 10.2 as well, because it writes the same archive: on
`create` always — the quota is already taken by the row that stays, but the bytes are not, because a
failed run carries `size_bytes = 0` (12.7), and on `restore` exactly when a new safety backup is
due, that is, together with `backup_limit_reached`. The refusal comes before the broken file is
cleared away and before the operation is created.

### 10.8 `GET /api/v1/servers/:id/backups/:backup_id/download`

Permission: **`BACKUPS`**. The file contains the complete server content, `BASE_READ` is not
enough. Response `200`, `Content-Type: application/zstd`, `Content-Length` set.

`Content-Disposition: attachment; filename="<slug>-<created_at>.tar.zst"; filename*=UTF-8''<…>`.
The name must **not** go raw into the header: up to 128 characters of free user input, and a quote
or a line break either breaks the header apart or slips a second one in. `<slug>` is the name
reduced to `[A-Za-z0-9._-]`, everything else to `-`, shortened to 64 characters, empty → `backup`;
the full name goes percent-encoded into `filename*`.

Errors: `409 backup_not_downloadable` (`status != "done"`), `409 backup_lives_in_drive` for
`location: "drive"` — the panel transfers no byte there, the path is `drive_web_link` (22.19).

### 10.9 `GET /api/v1/servers/:id/backups/schedule` · 10.10 `PUT …/schedule`

Permission: `BASE_READ` and `BACKUPS` respectively. Response `200` `BackupSchedule`; `PUT` takes
`UpdateBackupScheduleRequest` and answers with the same body including a freshly computed
`next_run_at`.

Limits: `interval_hours` 1–168; `hour_utc` 0–23, only evaluated for `interval_hours % 24 == 0`;
`keep_last` 1–50 and ≤ `max_backups`. Errors: `400 invalid_schedule` with plain words in `message`.

**Automatic backups are not in the plan**, but the interface visibly distinguishes them: filter
pills "Manual"/"Auto", an icon of their own, an "Auto" badge, a fallback text of their own "Backup
schedule" instead of "Manual backup". Without a schedule those are four dead branches and a filter
pill that never finds anything. The scope is deliberately small: interval in hours plus time of day,
**no cron**, plus `keep_last`; default **off**.

Cleanup happens **only** among automatic backups — a schedule rule may never delete a manually
created one. If nothing has changed since the last automatic one, it is skipped
(`last_status: "skipped_unchanged"`); the comparison is against the **completion** time of the last
automatic backup, not against `created_at` (that is the queuing time, and every file the server
touches while packing would be younger than it), and only over the backed-up set — otherwise
`logs/latest.log` alone would keep every server permanently "changed".

**`next_run_at` is written before the work starts, and also when nothing comes of it.** A backup of
a large server easily outlasts a tick; as long as the schedule does not say when the next time is
due, the server keeps reading as due, and the next tick would find it again. For the same reason a
tick that leads to nothing — no space, another run holds the server, nothing changed — also moves
the schedule forward: otherwise it would try the same thing every minute from then on and write the
same line into the log (`backups/schedule.rs:124-126`, counter-test `:394-396`).

### 10.11 `GET /modrinth/v0/backups/:backup_id/download`

The same response as 10.8, the same permission. It exists only because `BackupItem.vue:116` builds
the URL by hand and we adopt the component unchanged. The server ID is not in the path: the backup
ID is a ULID and globally unique, and the server is looked up from it. The parameter `?auth=` is
**ignored**; authorization goes through the session cookie. That way no secret ends up in the
history.

This path too answers `409 backup_lives_in_drive` for `location: "drive"` (22.19) — the same
response as 10.8, because it is the same response.

For the menu item not to stay grayed out, the component needs the properties `kyros-url` and `jwt`;
Modrinth feeds them from `server.node`. Our rebuilt page sets them directly to `location.host` and
`"cookie"` — so we need no `node` in the server object. Known limitation: `BackupItem.vue:116`
hardcodes `https://` into the URL; a panel on `http://192.168.1.10:8080` gets a broken link, and
there the only option left is to leave out the two properties and keep the menu item disabled.

### 10.12 Quota

`max_backups` per server, default **10**, changeable by the admin, at most 50. `Server.backup_quota`
carries exactly that value, `used_backup_quota` the number of **all** backup rows including the
running ones — `backups.vue:624` counts `backups.value.length`, not only the finished ones; whoever
counts only `done` in the backend gets a display that does not match the refusal.

One consequence you have to know: safety backups from 10.6 are `automated: false` and are **not**
cleaned up by `keep_last`. Whoever restores often fills up his quota that way, and the next restore
fails on `backup_limit_reached` through no fault of his own. The way out is to delete by hand: the
price for a cleanup rule never touching a non-automatic backup.

Drive backups **do** count against `max_backups`. They have to: if they did not, "50 local and 50 in
the Drive" would be a way around this quota, and `used_backup_quota` is the number of all rows
anyway. Against the byte budget they do **not** count (22.18) — the bytes do not lie on this
machine.

There is a **byte budget per user**: `disk_mib` in `UserLimits` (12.7) counts all of an account's
servers and their backups together. It applies at the panel paths and not in the kernel — cgroups
do not limit disk space, and the brake against a full disk from 10.2 stays next to it: one protects
the machine, the other divides it up. The `size_bytes` of the rows are the source of the backup
share; a running backup carries `0` until it ends and is therefore invisible by its own size for
that time.

---

## 11. Per-server access

### 11.1 `GET /api/v1/servers/:id/members`

Permission: `BASE_READ`. Response `200` `ServerMemberList`.

The owner is always the first entry and always present — he is not a real member row but is
generated from `server.owner_id`. His `id` is the **server ULID**: `id` is mandatory (`row-key` of
the table, `AccessTable.vue:9`), there is no member row it could come from, and it cannot collide
with a member ULID. Panel admins do **not** appear in the list (1.10).

**Two rows may nevertheless be standing there, and they are hidden rather than deleted.** 12.6 hands
a server over and leaves the member rows lying, so the new owner can sit in his own member list.
12.5 promotes a member to panel admin and likewise leaves his row standing; from then on it says
nothing true any more, because he holds `SERVER_ADMIN` everywhere no matter what is written there,
and the role field could take nothing of it away from him. Both are therefore left **out of the
list** instead of being removed: if the one is demoted again or the other gives up the server, the
row means again what it meant before. And 11.2 writes none in the first place for a panel admin, for
the same reason (`api/access.rs:86-98`, counter-tests `:910-913`, `:949-952`, `:775-777`).

The table is camelCase, our API snake_case; our page does the conversion (around 35 lines, modeled
on `access.vue:237-272`). Four fields are optional according to the type but needed in truth:
`invite_resend_available_at` (locks the resend button and labels it "Resend in {seconds}s",
`AccessTable.vue:548-568`), `pending` (switches to resend and revoke instead of remove access,
`:77,94-97`), `is_owner` (without the field a role select appears for the owner instead of a fixed
badge, `:34-41`), and `avatar_url` has to become `undefined` in the conversion, not `null` — the
contract wants `string | undefined`.

### 11.2 `POST /api/v1/servers/:id/members`

Permission: `MANAGE_USERS`. Request `AddMemberRequest`, response `201` `ServerMember` with
`pending: true`, `joined_at: null`.

**Invitations need an acceptance.** Before that the invitee has no access:
`current_user_permissions` is empty for him, and the server appears only in his invitation list.
Without acceptance "resend" would be pointless, and the interface shows "Pending", revoke and resend
hardwired.

Errors: `404 user_not_found`, `409 already_member` (also for an open invitation),
`400 cannot_invite_self`, `400 role_not_assignable` (for `"owner"`), `400 invalid_role`.

### 11.3 `PATCH /api/v1/servers/:id/members/:user_id`

Permission: `MANAGE_USERS`. Request `UpdateMemberRequest`, response `200` `ServerMember`. Takes
effect immediately, for an open invitation too.

If a role is lowered while the person affected is connected, his WebSockets are **not** closed (the
bits the socket needs are still there with `BASE_READ`); the individual actions are refused on the
next request.

**Roles are presets, not freely settable bits.** The API takes `role` and returns `permissions`;
setting individual bits would be possible but has no interface — the table knows a select with three
values.

Errors: `404 member_not_found`, `400 role_not_assignable`, `400 invalid_role`.

### 11.4 `DELETE /api/v1/servers/:id/members/:user_id`

Permission: `MANAGE_USERS`, **or** the caller removes himself. Response `204`. One and the same call
for removing and revoking — the interface distinguishes them only in the label and calls `delete` in
both cases (`access.vue:511-513,545-547,554`).

Side effect: all WebSockets of this user to this server close with `4403`.

Errors: `404 member_not_found`, `400 cannot_remove_owner`.

### 11.5 `POST /api/v1/servers/:id/members/:user_id/reinvite`

Permission: `MANAGE_USERS`. No body, response `200` `ReinviteResponse`. Within the cooldown the
endpoint answers **`200` with `sent: false`** and the remaining wait time, not with an error — that
is exactly how the interface reads it (`access.vue:488-492`). Cooldown 120 seconds.

**What "send" means here:** We have no mail delivery. The call refreshes `last_invite_sent`, nothing
more. The button therefore stays a gesture without a recipient; the cooldown is the only thing that
visibly happens.

Errors: `404 member_not_found`, `409 already_member` (invitation long since accepted).

### 11.6–11.8 Invitations

`GET /api/v1/invitations` — permission: a session; returns your own open invitations as
`InvitationList`. `id` is the same ID as `member.id` in 11.1 — an open invitation *is* a member row
without `joined_at`.

`POST /api/v1/invitations/:invitation_id/accept` — permission: a session, and the invitation has to
be addressed to the caller. No body, response `200` `ServerMember` (`pending: false`).

`POST /api/v1/invitations/:invitation_id/decline` — likewise, response `204`. The member row
disappears; a new invitation is possible afterwards.

Errors: `404 invitation_not_found` (also for someone else's invitation), `409 already_member`.

### 11.9 `GET /api/v1/servers/:id/audit-log`

Permission: `BASE_READ`. Query: `limit` (200, 1…500), `offset` (0), `order` (`desc`),
`min_datetime`, `max_datetime`, `actor` (ULID, repeatable, OR-joined), `action` (repeatable,
OR-joined; unknown names → `400 invalid_request`). Response `200` `AuditLogPage`; `next_offset` is
`null` on the last page.

`limit`/`offset`/`order`/`min_datetime`/`max_datetime` have the same names as in Modrinth; we
replace their JSON-encoded `filter` parameter with repeated parameters — the caller is our own page,
and a JSON blob in the URL is needlessly hard to read and to cache.

The entries do **not** come directly out of JSON but out of `parseAuditEvent(entry, lookups)`
(`components/servers/access/events/parser.ts:40`), a file we adopt unchanged, so its input is our
contract, and that one is Archon-shaped. `users` contains every actor appearing in the page slice
**and** every user named in metadata; `addons`/`versions` come from the cache from 8.16 and may be
missing (the display then shortens to eight characters of ID). `world_id` is **always `null`**; the
world column is hidden with `show-world-column="false"`.

The table's free-text search works **client-side** on the pages already loaded
(`AuditLogTable.vue:446-469`) — hence the large page size. We therefore do not bind `query` and
`filters`; two filter paths for the same thing are a trap. `hasActiveExternalFilters`, on the other
hand, is mandatory as soon as filtering happens server-side: without this flag the table shows "no
activity yet" for an empty result instead of "nothing matches the filters".

**Only names from this catalog are rendered readably**; everything else ends up at "Unknown event".
Of Modrinth's 42 names we adopt 39 and leave out `changed_server_subdomain`, `server_plan_changed`
and `sftp_login`.

| Action | Required metadata | produced in |
|---|---|---|
| `server_created` | — | 4.2 |
| `server_started`, `server_stopped`, `server_restarted`, `server_killed` | — | 4.6 |
| `server_repaired`, `server_reset` | — | 9.15, 9.16 |
| `server_reallocated` | — | 9.4 — we use it for "`-Xmx` changed"; a name of its own does not exist |
| `console_cleared` | — | 6.2 |
| `console_command_executed` | `{ command }` | 6.1 |
| `changed_server_name` | `{ name }` | 4.4 |
| `user_invited`, `user_permission_modified` | `{ user_id, permissions }` | 11.2, 11.3 |
| `user_invite_revoked`, `user_removed` | `{ user_id }` | 11.4 |
| `addon_added`, `addon_disabled`, `addon_enabled`, `addon_deleted`, `addon_updated` | `{ addons: [{ addon_id, version_id }] }` | 8.3–8.7 |
| `addon_uploaded` | `{ file_names }` | 8.8 |
| `modpack_changed`, `modpack_unlinked` | `{ spec: … }` | 8.10–8.12 |
| `port_allocation_added`, `port_allocation_removed` | `{ port }` | 9.7, 9.9 |
| `loader_version_edited` | `{ new_loader, new_version }` — the key `new_version` **must** be present (`parser.ts:172`) | 9.14 |
| `game_version_edited` | `{ new_version }` | 8.14 |
| `server_properties_modified` | `{ properties: { … } }` | 9.2 |
| `startup_command_modified` | `{ command }` | 9.4 |
| `java_runtime_modified` | `{ vendor }` · `java_version_modified` `{ version }` | 9.4 |
| `file_uploaded`, `file_deleted`, `file_edited` | `{ path }` | 7.6, 7.8, 6.6 |
| `file_renamed` | `{ from, to }` | 7.5 |
| `backup_created`, `backup_restored`, `backup_deleted` | `{ id }` | 10.2, 10.6, 10.4 |
| `backup_renamed` | `{ id, from, to }` | 10.3 |

If a mandatory field is missing, the entry falls back to `UnknownEvent`. **Retention 180 days**,
after that a daily run cleans up; when a server is deleted its log disappears with it.

**Writing never owes the handler an error.** When a row is written, the deed has already been done;
a hiccup of the database must not turn a `204` into a `500`. A row that fails is logged and dropped
(`audit/mod.rs:17-19`). And the names for the IDs come **only** out of the cache from 8.16, never
off the network: a name we do not have is a shortened ID in the display and not a Modrinth call
while paging (`audit/mod.rs:144-147`, `audit/page.rs:287-288`).

**Two lists can hit SQLite's limit of 32,766 bound values per statement**, and the two are handled
differently (`audit/page.rs:22-26`):

* the **actors from the query string** are written by the caller. They are OR-joined, so a name
  repeated ten thousand times yields the same page and ten thousand bound values. More names than a
  statement can bind is a bad request (`400`, 1.7 "value out of range") — the answer to a filter
  nobody can page through must not be a `500`.
* the **addon IDs in the metadata of a page** are written by the log itself: a modpack installation
  names every mod it brought along, so a single entry can carry more IDs than a statement has room
  for. The lookup therefore happens in bites. Without that, a lookup that falls over would take the
  whole page with it — this server's log would answer `500` from then on, and forever
  (`audit/mod.rs:190-193`, `audit/page.rs:346-347`).

---

## 12. Administration

### 12.1 `GET /api/v1/admin/host`

Response `200` `HostCapacity`. `allocated` is the sum of the user limits (not of the `-Xmx`), so it
is what the admin has given away. If it exceeds `assignable_memory_mib`, the machine is
overcommitted; that is allowed and the admin's business, and the interface warns.

**Panel admins are in none of the `allocated` sums**, because nothing was promised to them;
`unlimited_users` says how many accounts are missing as a result. Without that number the sum reads
like the whole story. `disk_total_bytes` is the `statvfs` of the data directory and therefore
machine-wide; `assignable_disk_mib` is the upper bound of the disk slider in 12.7.

`allocated.cpu_cores` adds up unlike things: with `cpu_mode: "cap"` assigned cores are a ceiling,
with `"share"` a share without an upper bound. The sum is still a number, but it means something
only in pure cap mode — the display has to label that.

### 12.2 `GET /api/v1/admin/users`

Query `query` (username), `limit` (50, max 200), `offset`. Response `200` `AdminUserList`.

Cost of this response: `usage.*.used_*` comes from three cgroup files per user — negligible at a
two-digit user count, and the values are cached for 5 seconds.

### 12.3 `POST /api/v1/admin/users`

Request `CreateUserRequest` (`limits` and `must_change_password` omittable → `default_limits` from
12.10 and `true` respectively). Response `201` `PanelUser`.

`limits` together with `panel_role: "admin"` is `400 invalid_request`: an admin has no limits, and
taking numbers that do not apply would be the lie from 12.7. The row silently gets `default_limits`
all the same — that is what a demotion brings back. Its response carries `limits: null`.

On creation the service calls the helper with `create-user <id>` (`docs/PLAN.md:187`): system user
`craft-<id>`, directory `users/<id>/`, owner and `2770`, plus the cgroup `user-<id>` with the
limits. That takes milliseconds, which is why the normal case is `system_user.state: "ready"` in the
same response.

A failed helper call is **not** a `500`: the panel user is created, the response is `201` with
`state: "error"` and `error_message` in plain words. Anything else would leave a half-created row
behind. The user can sign in but cannot create servers
(`capabilities.blocked_reason = "system_user_not_ready"`); catching up goes over 12.9.

`email` is omittable and the only way to attach a manually created account to mail delivery at all:
without an address there is no "forgot password" for this account (21.7). What an admin enters here
counts as usable — there is no confirmation mail for a path a human with access to the panel has
walked. Errors for it: `400 invalid_email` and `409 email_taken` (the same address already sits on
an account or on an open application). An address that currently has an **application** running on
it is therefore expressly not free: that is a human waiting for a decision, and the path is to
approve or reject him (20.6), not to walk past him. The same works from the terminal with
`admin create --email` and `admin email` (21.9).

For **every** field of `CreateUserRequest` and `UpdateUserRequest` the interface has to have a
control. That is not a request: three times something in this area was built finished and
unreachable (21.4 without a button, `/registration-pending` without a path, `email` without a
field). The guard for it is `web/src/pages/admin/users-reachable.test.ts`, and it takes its list
from **these** structs in `crates/craftpanel/src/api/admin.rs`, not from the interface and not from
`web/src/api/types.ts`, because a yardstick taken from the thing being measured passes every test.

Errors: `409 username_taken`, `400 weak_password`, `400 invalid_request` (name 3–39 characters,
`[a-z0-9_-]`, lowercase; or `limits` with `panel_role: "admin"`).

### 12.4 `GET /api/v1/admin/users/:user_id`

Response `200` `AdminUserDetail`: `PanelUser` plus `owned_servers` (the basis of the delete dialog;
it also explains the number in `allocated_mib`) and `active_sessions`.

### 12.5 `PATCH /api/v1/admin/users/:user_id`

Request `UpdateUserRequest`, all fields omittable. Response `200` `AdminUserDetail`.

Changing the name does **not** change the system user: that one is named after the ID, precisely
because names change. Setting a password discards all sessions of the person affected and closes his
WebSockets with `4401`; an admin does **not** need his old password for that. This endpoint does not
change limits, 12.8 does.

The sentence that stood here until this round — "this is the path for 'forgot password', because we
have no email" — is **no longer true**. There are three paths now, and this is the crudest: it sets
a password a second human knows. The path for the user is 21.1, the path for the operator without
mail delivery is `craftpanel admin passwd` (21.9), and this one stays for the case where an admin
has to help an account in by hand. It too discards all open reset tokens of the account (21.8) and
sends `password_changed` if an address is on file.

`email` sets or clears the address. Three meanings, and all three are needed: **field absent =
unchanged**, `null` (or an empty string) **clears**, text **sets**. A cleared entry takes reset over
mail away from the account; that belongs in the confirmation dialog of the interface and not in a
footnote — it warns at the place of the field and writes it on the button. The address is trimmed
and lowercased as everywhere (20.10), and it has to be unique: errors for it `400 invalid_email`,
`409 email_taken` (another row in `users` **or** an open application holds it).

**A change of address invalidates the open reset tokens of the account** (21.8) — the link lies in
the mailbox the account has just left, and "change the address, then use the link from before" is
exactly the takeover move. Clearing the address does the same. The **sessions stay** standing: an
address is not a sign-in credential, and this endpoint ends sessions for a new password and for
nothing else. Saving the same address again — spelled differently too — is **not** a change,
otherwise a token would die on the second press of "Save".

Three meanings for one field demand something of the reader: `Option<Option<T>>` alone does **not**
carry them. `Option` reads `null` as `None`, and a missing field likewise — both would arrive as
`None`, and "clear the address" would silently mean "leave unchanged". What makes the difference is
a `deserialize_with` that only runs when the field is **there** at all (`api/admin.rs:173-176`). The
same form carries `api_key` in 19.3 and `client_secret` in 22.12.

**A `PATCH` is one change, not four.** Every field is weighed before any one is written, and the
writes run in a transaction: a body with a new name and a password of four characters has to leave
the account as it was. Before, the name stayed put after `400` had been answered
(`api/admin.rs:370-373`, counter-test `:1469-1470`).

**A role change has to take the kernel along**, and nobody runs it through 12.8. The role decides
whether the four numbers of the row apply at all: a promotion has to take the ceilings out of the
cgroup, a demotion has to write them back in. Without this second write the kernel would keep what
the account had before (`api/admin.rs:470-474`, counter-test `:1990-1991`). An account whose system
user never came about is **skipped** here: there is no cgroup, and the attempt would write "limits
were not applied" over exactly the reason 12.9 has to show (`api/admin.rs:505-515`, counter-test
`:2037-2039`). A helper that cannot reach the cgroup, incidentally, does **not** roll the role change
back — the role concerns the panel, and an account on a machine without cgroup delegation still runs
servers; 12.3 draws the same line on creation.

An admin may rename himself, set his own password and change `must_change_password` on himself; he
may **not** demote himself to `user` as long as he is the only admin — there is exactly one code for
that, `409 last_admin`, and the same holds if an admin would demote another last admin.

Errors: `409 username_taken`, `409 email_taken`, `400 weak_password`, `400 invalid_email`,
`409 last_admin`, `409 user_busy`.

### 12.6 `DELETE /api/v1/admin/users/:user_id`

The decision about the servers sits in the query, so that no body hangs on a `DELETE`
(`docs/PLAN.md:369-371`):

| Call | Effect |
|---|---|
| no parameter | `409 user_has_servers` as soon as the user owns a server; otherwise deletes right away |
| `?servers=delete` | servers gone along with directories and backups, then the system user gone |
| `?servers=transfer&transfer_to=<user_id>` | servers pass to the target user, then the system user gone |

Response `204`. **Condition in both cases: none of the user's servers is running** → otherwise
`409 servers_running`. The admin stops them beforehand; we kill nothing.

**The race in between.** Between the check "nothing is running" and the move there is work, and in
that time anyone with `POWER_ACTIONS` can start a server. A `chown -R` under a running Java process
is exactly the kind of bug you only see weeks later. That is why the call first sets the user to
`busy`; while that lasts, "start server", `POST /servers`, 12.5 and 12.8 answer `409 user_busy` for
this user.

Transferring means: `rename` of the directory and the database still within the call, `chown -R`
afterwards in the background, and the user stays `busy` until the end. If the target user runs over
his budget as a result, **the transfer happens anyway**; he counts as over the limit afterwards and
cannot start anything new. The admin ordered it expressly, and "nothing gets killed" holds here too.
The tree under `users/<id>/` is removed in the background — the response does not wait for `rm -rf`
of 40 GiB.

Errors: `403 cannot_delete_self`, `409 last_admin`, `409 user_has_servers`, `409 servers_running`,
`409 user_busy`, `400 invalid_transfer_target`.

### 12.7 and 12.8 `GET`/`PUT /api/v1/admin/users/:user_id/limits`

Response `200` `UserLimitsResponse` (`limits`, `usage`, `host`). `host` appears there a second time
so that the slider knows its upper bound without fetching 12.1 as well. `PUT` takes `UserLimits` as
a **complete replacement**, all five fields mandatory.

**A panel admin has no limits.** `GET` answers `limits: null` for him, and likewise
`usage.memory.limit_mib`, `usage.cpu.limit_cores`, `usage.pids.limit` and `usage.disk.limit_mib` are
`null`. `PUT` answers `409 role_unlimited`: there is nothing to write, so there is no form either.
The columns of the row stay filled all the same — they are what a demotion brings back, and nobody
is measured against them as long as the account is an admin.

Implementation in the cgroup `user-<id>`:

| Field | cgroup |
|---|---|
| `memory_mib` | `memory.high` = value, `memory.max` = value × 1.25 (the emergency brake) |
| `cpu_cores` with `cpu_mode: "cap"` | `cpu.max` = `round(cores × 100000) 100000` |
| `cpu_cores` with `cpu_mode: "share"` | `cpu.weight` = `clamp(round(cores / host_cores × 10000), 1, 10000)`, `cpu.max` = `max` |
| `pids_max` | `pids.max` |
| `disk_mib` | **no cgroup equivalent** — cgroup v2 knows no disk space (`io.max` is throughput), the check sits in the panel |
| panel admin | all four files carry `max` |

A field the helper does **not** get, it writes as `max` (`craftpanel-helper/src/cgroup.rs`). That is
how "no ceiling" reaches the kernel: leaving out a number empties the file, it does not leave an
earlier one standing. Of the row `cpu_mode: "share"` only half arrives there today — see 17.16.

**The disk limit is not a kernel limit.** It applies at the paths that lead through the panel, and
that is this list — every path on which a user's bytes get onto the disk is in it, or below it among
the exceptions:

| Door | Asked with |
|---|---|
| 4.2 create a server | `0` — a new server is empty |
| 7.8 write and upload a file | announced `Content-Length`, before the first byte |
| 7.9 unpack | unpacked size from the central directory of the archive |
| 8.6 update content | `0` — every entry replaces a file that is already counted |
| 8.7 install content | twice: `0` before the resolution, the sum of the `size` entries afterwards |
| 8.8 upload content | announced `Content-Length`, before the first byte |
| 8.10/8.11 install and update a modpack | `0`; for an uploaded pack before the first byte |
| 8.14 change the game version | `0` — only the operation knows the size of the replacement files |
| 10.2 back up | the estimate of the packing run, together with `507 no_space` |
| 10.6 restore | the estimate of the safety backup |
| 10.7 retry | as 10.2; for `restore` only when a new copy is due |

Four paths do **not** ask, each for a reason: a world that grows in the game grows beyond every
limit, because the game process writes as its own system user and does not ask the panel at all; the
**automatic** backup from 10.10 runs without a caller, so it has nobody it could answer `409` to,
and `BackupScheduleStatus` knows no value for it (`skipped_limit` means the count quota); a server
that is **transferred** during a user deletion (12.6) passes over even when the recipient goes over
as a result — the admin ordered it expressly; and 9.14/9.15/9.16 copy the server jar from the
panel-wide cache into the server directory without asking. The last one is an open gap and not an
intention: a reset (9.16) **frees** space, and an account that may do nothing any more still has to
be able to clean itself up — as long as that difference is not decided, none of the three asks.
Real enforcement would need file system project quotas; we do not promise that here.
`disk_mib` counts all servers of the account **and** their backups together; a backup that is
currently running carries `size_bytes = 0` until it ends and is therefore not yet counted.

**The important case: a limit below what is already assigned.** The call **succeeds**. It throws
nobody off and refuses nothing (`docs/PLAN.md:364-367`): what runs keeps running, `memory.high` only
throttles. The response shows `over_limit: true`, `POST /servers` and "start server" answer
`409 over_limit` from then on, and the person affected sees it in his own `GET /me`. He is free as
soon as `allocated_mib ≤ limit_mib`, so after deleting a server or lowering its `-Xmx`, **not** when
little memory happens to be in use.

The same holds for `disk_mib` below what already lies on the disk: nothing is deleted, no process
ended, `over_limit_dimensions` carries `"disk"`. What is then refused is only what is new — and
`capabilities.can_start_servers` stays **true** as long as only the disk is over, because a start
takes up no space. `can_create_servers` becomes false.

`over_limit_dimensions` carries `"memory"`, `"disk"`, both or nothing. CPU and processes are not
assigned in advance, so with them there is nothing to go over.

Errors: `400 invalid_request` (`memory_mib` < 512, `cpu_cores` ≤ 0, `pids_max` < 64,
`disk_mib` < 1024), `409 user_busy`, `409 role_unlimited` (the account is a panel admin).
**No** error when the machine is overcommitted — that is the admin's decision, visible in 12.1.

### 12.9 `POST /api/v1/admin/users/:user_id/system-user/retry`

No body. Only permitted for `system_user.state ∈ {error, provisioning}`. Calls the helper again with
`create-user <id>`, response `200` `AdminUserDetail`. Without this endpoint an account would stay
permanently unusable after a helper error, and the only way out would be to delete it and create it
again. Errors: `409 system_user_not_ready` if the second attempt fails as well.

### 12.10 and 12.11 `GET`/`PUT /api/v1/admin/settings`

Response `200` `PanelSettings`. This endpoint gathers up four gaps that three area contracts point
at without any of them having defined it: the **port pool** (9.7 hands out from it,
`docs/PLAN.md:333`), the switch **outbound services** (6.3 and 6.7 need it for
`409 external_services_disabled` and `shareDisabled`), the **operation width** (5.13) and the shared
**upload limit** (7.8, 8.8). Plus `default_limits` as the preset for 12.3 and `public_address` for
`net.ip`.

`public_address` is the only field of the server record that the machine does not know by itself: a
server binds on `0.0.0.0`, there is no per-server address, and the value is displayed anyway
(`network.vue:248`). The installer fills it on the first run with what it finds; a name instead of
an address is allowed. What we do **not** do: guess the address at runtime — neither the first
non-loopback address nor a callback to a foreign service would give the right thing behind NAT or a
reverse proxy.

Two switches come along with section 20: `registration_enabled` (default **off**) and
`registration_requires_approval` (default **on**, only takes effect if the first is on). Both
defaults are the closed door — an update must not silently open a running panel, and if the operator
opens it, the safe setting is that he sees every account beforehand.

**What `registration_enabled` means in 20.1 is not this switch alone**: sign-up is open when the
switch is on **and** mail delivery is set up (19.2, including the panel address for the links). The
panel offers nothing it cannot see through. If mail delivery is missing, that is not an error and
not a red tile — the "Sign-up" card says in one sentence what is missing and where to get it.

**Mail delivery expressly does not hang on `external_services_enabled`.** The switch is meant for
Modrinth and mclo.gs (6.3, 6.7); whoever turns it off does not want to share a crash analysis — he
does not want to unknowingly lose password recovery. The Drive endpoints (22), on the other hand,
do hang on it, just like playit (18), because there a foreign call is the thing itself.

`PUT` replaces completely; response `200` with the same body. Errors: `400 invalid_request` (port
pool the wrong way round, outside 1024–65535, or it would exclude ports already handed out).
`registration_requires_approval: false` together with `registration_enabled: false` is **not** an
error: the second question is then merely moot, and a form that may not send along a grayed-out
setting would be a trap.

---

## 13. The WebSocket protocol

**One socket per server**, `/api/v1/servers/:id/ws`. No second one, not even panel-wide. All
messages are JSON objects of the form `{"type": "...", …}`.

### 13.1 Client → server: nothing

**There is not a single message from the browser to the server.** Everything that triggers something
goes over HTTP: commands over 6.1, power over 4.6, aborting and dismissing over 5.4/5.5. That way
permission, state and length errors have a status code and a stable `error` code instead of
disappearing into a message the layout could not evaluate anyway
(`ConsoleManagerContext.sendCommand` returns `void`).

Consequence: there is no `error` message from the server to the client either — it would no longer
have a sender it could answer. Errors of the socket itself show up over the close code (13.6).

Second consequence, and it has to be set by hand: **the upper limit for incoming messages belongs
lowered.** The library's default is 64 MiB per socket — 64 MiB that a browser can make us hold for a
message we throw away anyway. Anything longer than a control frame is already more than this
protocol provides for. We read anyway, but only to notice the end (`api/ws.rs:28-29,50-52,188`).

### 13.2 Connection setup

In this order, immediately after the upgrade:

1. `server` — the server record.
2. `state` — the run state.
3. `operations` — the complete operation snapshot including `busy_reasons`.
4. `console_history_start`, then the `console` blocks, then `console_history_end`.
5. up to ten `stats` from the ring buffer, oldest first — **only when the server is running**. If it
   is stopped, the look back would be a lie: the graph would show the last ten seconds before the
   stop as if they were now. Then a single fresh probe goes out, and the provider's watchdog fills
   the rest with zeros.

**The case that shapes this protocol:** "create a server" takes one to two minutes, but the socket
hangs on a server ID that does not yet exist at the click on "Create". The answer in one sentence:
**the database row comes into being before the work, not after it** (4.2). From the `201` response
on, the socket is reachable, the server is in the list with `status: "installing"`, and the operation
exists. Three holes remain, and all three are plugged without a second socket:

* Between `201` and the connection setup a few hundred milliseconds pass. Because the first
  `operations` message is a complete snapshot and contains finished operations until they are
  dismissed, nothing is lost — even an operation that fails in that gap is delivered afterwards.
* The server list has no socket and gets none: it polls `GET /api/v1/operations` every five seconds
  as long as something is running (4.1). A second delivery path is not worth it for a spinner.
* Browser closed and opened later: the operation keeps running in the backend, and on coming back
  the list or the socket delivers the state. No state lives in the browser.

**The one precondition that really carries:** every operation belongs to exactly one server. There
is no operation without a server in this panel — which is why one socket per server is enough.

### 13.3 Twelve messages, server → client

| # | `type` | when | area |
|---|---|---|---|
| 1 | `server` | the server record has changed (name, port, status, loader after installation), plus on setup | 4 |
| 2 | `state` | the run state has changed, plus on setup | 4 |
| 3 | `stats` | tick see 13.4 | 4 |
| 4 | `operations` | complete snapshot, on every state change, plus on setup | 5 |
| 5 | `console_history_start` | first message of the history | 6 |
| 6 | `console` | history **and** live output | 6 |
| 7 | `console_history_end` | end of the history | 6 |
| 8 | `console_cleared` | buffer emptied (6.2 or a server start) | 6 |
| 9 | `content_changed` | update check done or an external change in `mods/` | 8 |
| 10 | `backup_list_changed` | after creating, renaming, deleting, dismissing, cleaning up | 10 |
| 11 | `startup_changed` | after every `PATCH /startup`, from others too | 9 |
| 12 | `network_changed` | after every change to the allocations | 9 |

Expressly **no** message for `server.properties`: the page reloads on every change of the run state
anyway (`properties.vue:447-449`), and two editors sitting on the same file at the same time are not
a case we want to solve. Whoever saves last wins.

All messages go to **all** sessions with `BASE_READ` on this server, not only to the triggering one —
otherwise a second editor sees a list that is no longer right, and his next click ends in a `404`.

### 13.4 `state` and `stats`

`state` carries `power_state`, `target`, `uptime_seconds`, `exit_code` and `oom_killed`.
`powerStateDetails` is set only for `power_state === "crashed"`, otherwise expressly `undefined`.
The provider counts `uptime_seconds` up locally once a second between two messages and sets it to 0
on `stopped`/`crashed`.

**`uptime_seconds` is true only for the moment in which the report came into being**, and every new
socket is handed the *stored* report. As long as the number in it was frozen, the uptime started
over at zero on every page reload. The channel therefore puts the point in time at which the server
came up next to the report, and computes the seconds freshly on every read (`ops/events.rs:170-174`,
counter-test `:661-679`). Across a **panel restart** this clock delivers nothing: a supervisor we
have merely found again has been running longer than the panel process exists. The anchor for that
is the column `servers.running_since` — the only record that outlives the process
(`ops/follow.rs:131-153`).

**`state` carries no installation progress.** Modrinth's `install`/`install_error` in this message is
replaced by `operations` — two progress reports for the same operation on one socket would be one
too many. The provider builds `SyncProgress` and `ContentError` for `InstallingBanner` out of the
operation: `phase` over 5.9, `percent` = `progress * 100`, `step` = `error.step`,
`description` = `error.message`.

`oom_killed` is a **guess**: the cgroup belongs to the user, not to the server
(`docs/PLAN.md:229-234`); we only establish that `oom_kill` in `memory.events` has risen while this
process ended through SIGKILL. With two servers of the same user dying at the same time, the
attribution can be wrong.

And it applies only when the SIGKILL did **not** come from the supervisor itself, so not at the end
of the signal ladder of a `kill` or a forced `stop` (4.6). The counter `oom_kill` belongs to the
whole account and never falls again; without this condition, after the first real memory overflow
every later kill of this user would have reported "crashed, memory overflowed" forever, including a
crash analysis (6.3) for something somebody did on purpose.

`stats` carries the five fields of `ServerStatsSample`. Tick:

* every second as long as `power_state === "running"` — below the 5-second threshold of the client
  watchdog (`server-manage-core-runtime.ts:65`) and matching the ten-point graph, which thereby
  shows ten seconds.
* with the server not running only every 30 s, because `storage_usage_bytes` keeps changing through
  file operations. The one-second tick is not needed: the watchdog pulls CPU and RAM to zero by
  itself and keeps the storage value.
* `storage_usage_bytes` is not measured freshly on every probe — a `du` over a modpack directory is
  expensive. A background run every 30 s fills a cache.
* WebSocket ping every 30 s at the protocol level, no message type of its own.

**The API transfers no history.** `graph.cpu` and `graph.ram` are pure client buffers with ten
points; `graph.ram` is not in bytes but `floor(ram_usage / ram_total * 100)`, and `padGraph` caps
at 100. Where the numbers come from:

| Field | Source |
|---|---|
| `cpu_percent` | increase of `utime+stime` over the process tree from `/proc/<pid>/stat`, divided by the elapsed wall-clock time **times the owner's CPU quota**, times 100. 100 % means: this server alone exhausts its owner's budget. If the owner has no quota (12.7), the denominator is the machine's core count |
| `ram_usage_bytes` | sum of the RSS over the process tree, read from `VmRSS:` in `/proc/<pid>/status`. **Not** from the cgroup — that one is set up per user and would give the sum of all his servers |
| `ram_total_bytes` | `Server.memory_mib × 1048576`, that is, exactly the `-Xmx` of this server. No plan, no machine size |
| `storage_usage_bytes` | size of the server directory, measured again every 30 s; backups do not count (10) |

**This number can be too small, and silently so.** The game process owns its own tree and may close
folders inside it — WorldEdit unpacks its languages as `drwx--S---` — and the panel gets in over the
group `craftpanel` or not at all (`docs/PLAN.md:196-205`). What lies behind such a door takes up disk
all the same. A walk that was refused a directory has therefore counted a **floor** and not a size.
13.4 and 12.7 both stand on this number, which is why there are two measurements: one that delivers
only the number, and one that additionally counts how many directories it was turned away at — not
to show them one by one, but so that "none" can be told from "a few" (`files/mod.rs:52-63,73-83`).
And both are something other than `statvfs` on the same path, which means the disk of the whole
machine and stands where the slider from 12.7 needs its upper bound (`files/mod.rs:97-99`).
| `storage_total_bytes` | `statvfs` of the file system, machine-wide and deliberately not a promise — at the same time the only field no component displays. **Not** the account's disk limit: that one is in `usage.disk` (3.3) and is a panel number per user, while this one here means the whole machine |

That `ram_usage_bytes` (RSS, includes off-heap and metaspace) can rise above `ram_total_bytes`
(`-Xmx`) is normal: the graph caps at 100, the tile shows the real value. Forcing a number below
100 % would need JMX in the server process — we reject that.

Four small things about reading `/proc`, each a mistake you make once
(`servers/manager.rs:2037-2131`):

* **`VmRSS:` from `status`, not `statm`.** `statm` counts pages, and you cannot find out the page
  size without a C library; `status` names kibibytes in plain text. The number is the same, the
  source is one that gets by without a `libc` call.
* **`utime`/`stime` are in USER_HZ, and that is 100**, no matter what the kernel ticks at. The
  constant is a property of the `/proc` interface, not of the machine.
* **`/proc/<pid>/stat` is only split up behind the closing parenthesis.** The command name stands in
  parentheses and may contain spaces; whoever counts the fields from the front counts wrong for a
  server called `java -jar my server.jar`.
* **The process tree is cut off at 512 entries.** A fork bomb is no reason to run forever for a
  metric — and the metric is only a display anyway.

### 13.5 `operations`, `console*` and the area messages

**`operations`** is **always the full state**, never a difference — Modrinth does the same
(`WSFilesystemOpsEvent.all`). A snapshot cannot arrive in the wrong order and needs no recovery
after a connection loss. Contained are all non-dismissed operations of this server, running **and**
finished — which is why a browser that connects only after the failure sees the red banner too. Pure
progress changes are throttled to **one message per second**; state and phase changes go out
immediately. `revision` and `busy_reasons` ride along (5.2, 5.10).

"Full state" nevertheless has an upper limit: **200 operations**, the same one 5.2 sets on the same
list over HTTP (`ops/store.rs:19-23`). The retention of seven days (5.12) keeps the snapshot small
anyway, but a message whose size no rule limits is a message that one day does not arrive.

And `revision` rises on **every** write to an operation, pure progress included
(`ops/store.rs:349-358`): a throttled snapshot whose number stayed unchanged would be thrown away by
the provider as already seen.

Out of the same message the provider also feeds `handleWsBackupProgress`
(`server-backups-queue.ts:71`): `backup_id` = `Operation.target_id`, `task` = `create`/`restore` out
of `kind`, `state` and `progress` directly. That is why there is no `backup_progress` message of its
own.

**`console`** carries history and live output; the difference lies solely in the bracketing by
`console_history_start`/`_end`. `lines` are complete lines **without** a line break, in order of
arrival, stdout and stderr merged. `seq` is the running number of the **first** line in the field;
it counts **per server**, not per server process, and is **never** reset — neither by
`console_cleared` nor by the start of a new process. The next
expected number is `seq + lines.length`; a jump forward means "lines lost", a jump backwards must
not occur (a counter per process produced exactly that on a restart and turned the loss indicator
into noise). Block size up to 500 lines or 64 KiB; live output is bundled every 100 ms.

**What carries "never reset" across a panel restart is the column `servers.console_seq`.** The
cleanup tick (every 30 s) writes the state of every open channel there, and does so with
`AND console_seq < ?`, so that a late-arriving writer cannot pull the number back
(`ops/mod.rs:366-379`). A channel that is needed for the first time starts at what stands in the
column (`ops/console.rs:34-36`).

**The channel per server holds 64 events and waits for no reader** (`ops/events.rs:23-27`). In the
worst case that is 64 console blocks of 64 KiB — four mebibytes per server, **once**, no matter how
many are watching. Whoever reads more slowly is told that he has missed something instead of being
waited for; what he sees is the jump in `seq`. A browser that stops reading can therefore neither
slow the supervisor down nor make the panel grow. What travels on the channel is serialized **once**
and shared — except for `server`, whose `current_user_permissions` is the mask of the respective
reader (`ops/events.rs:129-137`).

**The race between history and live output has to be won by the server, not by chance.** On setup
the end state of the ring buffer is captured **under the same lock** and the listener is registered
at exactly that place. What comes into being after that travels into the queue of this connection
and goes out **behind** `console_history_end`, never in between. Whoever registers first and reads
out afterwards delivers lines twice; whoever reads out first and registers afterwards loses the ones
in between.

`console_history_start` carries `total_lines` and `dropped_lines` (evicted from the ring buffer;
anything other than 0 means the history is incomplete). The provider sets `loading = true` and clears
its state, so there are no duplicates after a connection loss. With that we solve exactly what
Modrinth noted as a shortcoming in the layout (`console/layout.vue:226`) and guesses at today with
two timers.

**No `after_seq` resume.** After a loss the whole buffer comes again; that is a deliberate sacrifice
in favor of a socket without client state. `seq` is in the message all the same, so that a later
resume does not become a break in the format.

Ring buffer per server: **10,000 lines or 4 MiB**, whichever comes first (the start of a large
modpack produces 3,000–6,000 lines; at ~100 bytes per line that is ~1 MB per running server). If it
is empty — the panel just started, or the server is stopped — it is pre-filled from the last 10,000
lines from the **end** of `logs/latest.log`; without this priming the console of a stopped server
shows nothing and the crash message would stand above an empty terminal. **Only** the end is read
here and never the whole file (`ops/console.rs:137-171`): it belongs to the account the game runs
under, so its size is that account's decision, and reading it in whole would mean handing it the
panel's memory — the ring it fills holds only four mebibytes anyway. Because a window by bytes
begins in the middle of a line, the first partial line is dropped; half a line is not a line. And a
single byte no decoder reads must not empty the history: reading is lossy. Beyond that the browser
collects up to **25,000 lines or 8 MiB** (the limit comes from mclo.gs, 6.7).

`console_cleared` comes on 6.2 **and** on the start of a server process; that keeps the output of two
runs from running into each other without a recognizable seam. The seam is the message, not a jump in
the counter. If it arrives **during** the history, it applies immediately: the provider discards
everything received up to then and treats the following blocks as a new beginning;
`console_history_end` comes afterwards as planned.

**`content_changed`** carries `reason`: `updates_checked` or `external_change`. **No**
`task_finished` — that an operation is finished stands in the `operations` snapshot, and a second
trigger would mean reloading twice. The provider thereupon reloads 8.1, **unless** `isBulkOperating`
is set: a reload in the middle of a bulk run would pull the rows out from under the user.

**`backup_list_changed`** is without payload. The recipient reloads 10.1 **and the server details**,
because `used_backup_quota` hangs there. Modrinth knows five separate events for this but treats them
all alike — five messages for one effect are four too many.

### 13.6 Close codes

| Code | when | effect in the provider |
|---|---|---|
| `1000` | normal, page left | `isConnected = false` |
| `1012` | the service is restarting | reconnect with backoff |
| `4401` | no session or an expired one, signed out, password changed | `isWsAuthIncorrect = true`, do **not** reconnect, to the sign-in screen |
| `4403` | no `BASE_READ` (any more), access removed, invitation revoked, foreign `Origin` | the same flag, plus a return to the server list |
| `4404` | server finally deleted (operation `server_delete` finished) | clean up and to the list |
| `4429` | more than four sockets for the same session and the same server | do not reconnect |

`isWsAuthIncorrect` is a mandatory field of the server context; Modrinth feeds it from the socket
events `auth-incorrect`/`auth-ok`. That runtime hangs on the Archon client and is not adopted — with
us the flag comes out of the close code, and an additional pair of messages becomes unnecessary.

**Where `4401` and `4403` on an open socket come from at all: from asking again.** A permission can
be removed while the socket is standing — a membership that was deleted, a server that is gone, a
session that was signed out or whose password was changed, and **nobody tells us**. Asking again on
a slow tick is the whole mechanism; it costs two queries a minute. The two cases stay separate in
this: a session that is gone is `4401` and sends the browser to sign-in, a permission that is gone is
`4403` and sends it to the server list (`api/ws.rs:25-26,236-241`).

---

## 14. All data types

One contiguous block, to be copied verbatim into `web/src/api/types.ts`. It is valid on its
own: no imports, no references to `@modrinth/*`. Where a name would collide with a vendor
type, ours carries the prefix `Api`.

```ts
/* ======================= 1. Basics ======================= */

/** ULID, 26 characters of Crockford base32. */
export type Ulid = string
/** RFC 3339 in UTC, e.g. "2026-08-12T14:03:11Z". */
export type Rfc3339 = string
/** Root-relative, POSIX; on send the leading '/' is optional, on receive it is always there. */
export type FilePath = string

export interface ApiError {
	error: string
	message: string
}

/* ======================= 2. Roles and permissions ======================= */

export type PanelRole = 'admin' | 'user'

/** Identical to ServerAccessRole, components/servers/access/types.ts:5 */
export type ServerRole = 'owner' | 'editor' | 'viewer'

/** The ten bits we keep. Names verbatim as in
 *  composables/server-permissions.ts:15-32 — unknown ones are silently dropped there. */
export type PermissionBit =
	| 'BASE_READ'
	| 'POWER_ACTIONS'
	| 'EXEC_COMMANDS'
	| 'FILES_WRITE'
	| 'SETUP'
	| 'BACKUPS'
	| 'ADVANCED'
	| 'RESET_SERVER'
	| 'MANAGE_USERS'
	| 'SERVER_ADMIN'

/** Bit names joined by ' | '. Empty string = no permissions.
 *  Not an array: parsePermissionString calls value.split('|'). */
export type PermissionMask = string

export interface UserRef {
	id: Ulid
	username: string
	/** We have no avatar images; always null. */
	avatar_url: string | null
}

/* ======================= 3. Operations ======================= */

export type OperationKind =
	| 'server_create'
	| 'server_delete'
	| 'install_loader'
	| 'repair_content'
	| 'reset_server'
	| 'install_modpack'
	| 'install_content'
	| 'update_content'
	| 'change_game_version'
	| 'install_java'
	| 'backup_create'
	| 'backup_restore'
	| 'unarchive'

/** 'failed' and not 'error': six check sites write state.startsWith('fail'). */
export type OperationState = 'queued' | 'ongoing' | 'done' | 'failed' | 'cancelled'

export type OperationPhase =
	| 'analyzing'
	| 'installing_loader'
	| 'verifying'
	| 'running_installer'
	| 'installing_pack'
	| 'addons'
	| 'writing_config'

export type OperationErrorStep = 'modloader' | 'modpack' | 'download' | 'filesystem' | 'internal'

export interface OperationError {
	/** Stable and machine-readable, list in 5.11. */
	code: string
	/** For humans; becomes ContentError.description. */
	message: string
	/** Becomes ContentError.step. */
	step: OperationErrorStep
}

export interface Operation {
	id: Ulid
	server_id: Ulid
	kind: OperationKind
	state: OperationState
	/** Set only for the install kinds. */
	phase: OperationPhase | null
	/** 0…1, never 0…100. */
	progress: number
	/** Free text for our own interface. No vendor component reads it. */
	message: string | null
	/** Required for unarchive (archive path), otherwise null. */
	src: FilePath | null
	bytes_processed: number | null
	files_processed: number | null
	current_file: string | null
	error: OperationError | null
	/** May flip to false mid-run once the point of no return passes. */
	cancellable: boolean
	/** backup_id for backup_create and backup_restore, otherwise null. */
	target_id: Ulid | null
	/** User ID; null when the panel started the operation itself. */
	started_by: Ulid | null
	created_at: Rfc3339
	started_at: Rfc3339 | null
	finished_at: Rfc3339 | null
	/** null = not dismissed yet; corresponds to should_prompt === true. */
	dismissed_at: Rfc3339 | null
}

export type BusyReasonCode =
	| 'installing'
	| 'syncing_content'
	| 'backup_creating'
	| 'backup_restoring'
	| 'deleting'

export interface OperationListResponse {
	/** Monotonic per server; older snapshots are discarded. */
	revision: number
	operations: Operation[]
	busy_reasons: BusyReasonCode[]
}

export interface AllOperationsResponse {
	operations: Operation[]
	busy_reasons_by_server: Record<Ulid, BusyReasonCode[]>
}

/** Response of every endpoint that starts an operation. Always 202, except for POST /servers. */
export interface OperationAccepted {
	operation: Operation
}

/* ======================= 4. Server ======================= */

export type ServerStatus = 'installing' | 'available' | 'broken' | 'deleting'

export type LoaderId =
	| 'vanilla'
	| 'paper'
	| 'folia'
	| 'purpur'
	| 'leaf'
	| 'fabric'
	| 'velocity'
	| 'neoforge'
	| 'quilt'
	| 'forge'

export interface ServerNet {
	/** From admin_settings.public_address; null when nothing is set. */
	ip: string | null
	port: number
	/** Always "": ServerSubdomainLabel.vue:18 hard-appends '.modrinth.gg'. */
	domain: string
}

export interface ServerUpstream {
	kind: 'modpack'
	project_id: string
	version_id: string
}

export interface Server {
	id: Ulid
	name: string
	owner_id: Ulid
	status: ServerStatus
	game: 'Minecraft'
	/** Lowercase; the display name lives in LoaderInfo.name. */
	loader: LoaderId | null
	loader_version: string | null
	/** The adapter turns this into Archon.Servers.v0.Server.mc_version. */
	game_version: string | null
	net: ServerNet
	memory_mib: number
	upstream: ServerUpstream | null
	/** true after reset-to-setup and after a failed server_create. */
	flows: { intro: boolean }
	/** max_backups of this server; ServerListing and backups.vue read it. */
	backup_quota: number
	/** Count of all backup rows, including the running ones. */
	used_backup_quota: number
	update_channel: UpdateChannel
	current_user_permissions: PermissionMask
	created_at: Rfc3339
}

export interface ServerListResponse {
	servers: Server[]
	users: Record<Ulid, UserRef>
}

/** Verbatim Modrinth's PropertiesFields; the wizard builds exactly this shape
 *  (creation-flow-context.ts:524-541). */
export interface KnownProperties {
	allow_cheats?: string | null
	allow_flight?: string | null
	difficulty?: string | null
	enforce_whitelist?: string | null
	force_gamemode?: string | null
	gamemode?: string | null
	generate_structures?: string | null
	generator_settings?: string | null
	hardcore?: string | null
	level_seed?: string | null
	level_type?: string | null
	max_players?: string | null
	max_tick_time?: string | null
	motd?: string | null
	pause_when_empty_seconds?: string | null
	player_idle_timeout?: string | null
	require_resource_pack?: string | null
	resource_pack?: string | null
	resource_pack_id?: string | null
	resource_pack_sha1?: string | null
	simulation_distance?: string | null
	spawn_protection?: string | null
	sync_chunk_writes?: string | null
	view_distance?: string | null
	white_list?: string | null
}

export interface PropertiesFields {
	known: KnownProperties
	custom?: Record<string, string>
}

export type CreateServerContent =
	| {
			kind: 'loader'
			loader: LoaderId
			game_version: string
			/** null = latest stable build. */
			loader_version: string | null
	  }
	| { kind: 'modpack_project'; project_id: string; version_id: string }
	| { kind: 'modpack_upload'; file_name: string; file_size: number }

export interface CreateServerRequest {
	name: string
	/** Only a panel admin may set this to something other than null. */
	owner_id: Ulid | null
	memory_mib: number
	/** Only a panel admin may set this to something other than null. */
	port: number | null
	eula_accepted: boolean
	content: CreateServerContent
	properties: PropertiesFields
}

export type ServerWarning = 'memory_overcommitted' | 'properties_will_be_ignored'

export interface CreateServerResponse {
	server: Server
	operation: Operation
	warnings?: ServerWarning[]
}

export interface UpdateServerRequest {
	name?: string
	update_channel?: UpdateChannel
}

export type PowerAction = 'start' | 'stop' | 'restart' | 'kill'
export type PowerState = 'stopped' | 'starting' | 'running' | 'stopping' | 'crashed'
/** kill takes effect at once and never stays behind as a target. */
export type PowerTarget = 'start' | 'stop' | 'restart'

export interface PowerRequest {
	action: PowerAction
}

export interface PowerResponse {
	power_state: PowerState
	target: PowerTarget | null
}

/* ======================= 5. Console ======================= */

export interface SendCommandRequest {
	command: string
}

export type CrashAnalysisSource = 'latest_log' | 'buffer'

export interface CrashAnalysisRequest {
	source?: CrashAnalysisSource
}

export interface CrashAnalysisEntry {
	level: number
	time: string | null
	prefix: string
	lines: Array<{ number: number; content: string }>
}

/**
 * Response of POST https://api.mclo.gs/1/analyse, trimmed down to `success` and `entries`.
 * Deviation from Modrinth's type on purpose: `name` and `version` are nullable — the real
 * API returns null there as soon as it does not recognize loader or version (measured 2026-08-12).
 */
export interface CrashAnalysisResponse {
	id: string
	name: string | null
	type: string
	version: string | null
	title: string
	analysis: {
		problems: Array<{
			message: string
			counter: number
			entry: CrashAnalysisEntry
			solutions: Array<{ message: string }>
		}>
		information: Array<{
			message: string
			counter: number
			label: string
			value: string
			entry: CrashAnalysisEntry
		}>
	}
}

export type LogFileKind = 'log' | 'crash_report'

export interface LogFile {
	file: FilePath
	name: string
	kind: LogFileKind
	size_bytes: number
	modified_at: Rfc3339
	compressed: boolean
}

export interface LogFileListResponse {
	/** Count before the cap. */
	total: number
	truncated: boolean
	files: LogFile[]
}

export interface LogFileContentResponse {
	file: FilePath
	/** File on disk. */
	size_bytes: number
	/** Length of content; a different number for .gz. */
	content_bytes: number
	/** cut off at the front. */
	truncated: boolean
	content: string
}

export const CONSOLE_SERVER_BUFFER_LINES = 10_000
export const CONSOLE_SERVER_BUFFER_BYTES = 4 * 1024 * 1024
export const CONSOLE_CLIENT_BUFFER_LINES = 25_000 // = mclo.gs maxLines
export const CONSOLE_CLIENT_BUFFER_BYTES = 8 * 1024 * 1024 // < mclo.gs maxLength
export const CONSOLE_HISTORY_CHUNK_LINES = 500
export const CONSOLE_MAX_LINE_BYTES = 8192
export const CONSOLE_MAX_COMMAND_BYTES = 8192
export const LOG_GUNZIP_MAX_BYTES = 512 * 1024 * 1024
/** [HH:MM:SS] [Panel/INFO]: … — /INFO, /WARN, /ERROR drive the level (log-level.ts:3-12). */
export const PANEL_LINE_TAG = 'Panel'

/* ======================= 6. Files ======================= */

/** Identical to FileItem from files-tab/types.ts:1 — directly assignable. */
export interface ApiFileItem {
	name: string
	type: 'file' | 'directory' | 'symlink'
	path: FilePath
	/** Unix seconds. Binding through FileTableRow.vue:303. */
	modified: number
	/** Unix seconds, 0 when the file system keeps no birth time. */
	created: number
	/** Only for type === 'file'. */
	size?: number
	/** Only for type === 'directory'. */
	count?: number
	/** Only for type === 'symlink'; raw link content. */
	target?: string
}

export interface FilesMetaResponse {
	root_path: string
	max_upload_bytes: number
	max_text_bytes: number
	max_page_size: number
	default_page_size: number
	max_extract_uncompressed_bytes: number
	max_extract_entries: number
}

export interface ListDirectoryQuery {
	path?: FilePath
	/** Name of the last entry of the previous page, exclusive. */
	after?: string
	page_size?: number
}

export interface ListDirectoryResponse {
	path: FilePath
	page_size: number
	/** Snapshot; no consumer in the interface. */
	total: number
	has_more: boolean
	next_after: string | null
	items: ApiFileItem[]
}

export interface CreateItemRequest {
	path: FilePath
	type: 'file' | 'directory'
}

export interface CreateItemResponse {
	item: ApiFileItem
}

export interface MoveItemRequest {
	source: FilePath
	/** Full destination path including the file name. */
	destination: FilePath
	overwrite?: boolean
}

export interface MoveItemResponse {
	moved: boolean
}

export interface DeleteItemQuery {
	path: FilePath
	recursive?: boolean
}

export interface ReadContentQuery {
	path: FilePath
	max_bytes?: number
	download?: 0 | 1
}

export type WriteConflictMode = 'overwrite' | 'fail'

export interface WriteContentQuery {
	path: FilePath
	on_conflict?: WriteConflictMode
}

export interface ExtractRequest {
	path: FilePath
	/** null or omitted = the archive's directory. */
	target?: FilePath | null
	override: boolean
	dry: boolean
}

/** Identical to ExtractDryRunResult from files-tab/types.ts:64. */
export interface ExtractDryRunResponse {
	modpack_name: string | null
	/** At most 200 entries. */
	conflicting_files: FilePath[]
}

/* ======================= 7. Content ======================= */

export type ContentProjectType = 'mod' | 'plugin' | 'datapack' | 'resourcepack' | 'shader'
export type ContentSourceKind = 'local' | 'modrinth_modpack' | 'server_project'
export type UpdateChannel = 'release' | 'beta' | 'alpha'

export interface ModrinthOwner {
	id: string
	name: string
	type: 'user' | 'organization'
	avatar_url: string | null
}

export interface ContentProject {
	id: string
	slug: string | null
	title: string
	icon_url: string | null
}

export interface ContentVersion {
	id: string
	version_number: string
	file_name: string
	date_published: Rfc3339 | null
}

export interface ApiContentItem {
	/** ULID of our database row — stable across renaming and updating (8.1). */
	id: Ulid
	file_name: string
	file_path: FilePath
	size: number
	enabled: boolean
	/** true for the loader jar and the server core. */
	locked: boolean
	project_type: ContentProjectType
	date_added: Rfc3339
	source_kind: ContentSourceKind
	/** Labrinth.Versions.v3.Version.environment; null leaves out the warning triangle. */
	environment: string | null
	/** Always false: client mods are never laid down in the first place (8.17). */
	pack_client_retained: boolean
	pack_client_depends: boolean
	installing: boolean
	external: boolean
	external_url: string | null
	has_update: boolean
	update_version_id: string | null
	/** From our own row; null only for a file with no recognized origin (8.1). */
	project_id: string | null
	/** null allowed — a cache; the provider fills it in then (8.1). */
	project: ContentProject | null
	version: ContentVersion | null
	owner: ModrinthOwner | null
}

export interface ContentModpack {
	source_kind: 'modrinth_modpack' | 'local'
	project_id: string | null
	slug: string | null
	title: string
	description: string | null
	icon_url: string | null
	filename: string | null
	downloads: number | null
	followers: number | null
	owner: ModrinthOwner | null
	/** Strings; the provider builds the objects for ContentModpackCard. */
	categories: string[]
	version_id: string | null
	version_number: string | null
	date_published: Rfc3339 | null
	has_update: boolean
	update_version_id: string | null
}

export interface ContentListResponse {
	content_type: ContentProjectType
	loader: LoaderId
	loader_version: string | null
	game_version: string
	update_channel: UpdateChannel
	updates_checked_at: Rfc3339 | null
	permissions: { can_read: boolean; can_write: boolean }
	modpack: ContentModpack | null
	items: ApiContentItem[]
	/** true when the ceiling of 2,000 items has taken hold. */
	truncated: boolean
}

export interface ModpackContentsResponse {
	items: ApiContentItem[]
}

export interface ContentIdsRequest {
	ids: Ulid[]
}

export interface ContentMutationResult {
	id: Ulid
	ok: boolean
	file_name: string | null
	file_path: FilePath | null
	enabled: boolean | null
	error: string | null
	message: string | null
}

export interface ContentMutationResponse {
	results: ContentMutationResult[]
}

export interface ContentUpdateTarget {
	id: Ulid
	version_id: string | null
}

export interface ContentUpdateRequest {
	items: ContentUpdateTarget[]
	all: boolean
}

export interface ContentUpdateResponse {
	operation: Operation
	/** Denominator of the progress bar: BulkOperationStatus.progress is an item count. */
	total: number
}

export interface ContentInstallTarget {
	project_id: string
	version_id: string | null
}

export interface ContentInstallRequest {
	items: ContentInstallTarget[]
	resolve_dependencies: boolean
}

/** The same values as Modrinth's resolver, labrinth/types.ts:41-50. */
export type ContentSkipReason =
	| 'already_installed'
	| 'duplicate_project'
	| 'conflicting_dependency'
	| 'no_compatible_version'
	| 'missing_version'
	| 'quilt_fabric_api'

export interface ContentPlanEntry {
	project_id: string
	version_id: string
	file_name: string
	reason: 'requested' | 'dependency'
}

export interface ContentSkippedEntry {
	project_id: string
	version_id: string | null
	reason: ContentSkipReason
}

export interface ContentInstallResponse {
	operation: Operation
	planned: ContentPlanEntry[]
	skipped: ContentSkippedEntry[]
}

export interface ContentUploadResult {
	file_name: string
	ok: boolean
	id: Ulid | null
	error: string | null
	message: string | null
}

export interface ContentUploadResponse {
	results: ContentUploadResult[]
}

export interface ContentDependentEntry {
	id: Ulid
	depends_on: Ulid[]
}

export interface ContentDependentsResponse {
	dependents: ContentDependentEntry[]
}

export type ModpackSource =
	| { kind: 'modrinth'; project_id: string; version_id: string | null }
	| { kind: 'upload' }

export interface ModpackInstallRequest {
	source: ModpackSource
	keep_extra_content: boolean
}

export interface ModpackUpdateRequest {
	version_id: string | null
}

export interface ModpackUnlinkResponse {
	unlinked: boolean
	adopted_items: number
}

export type GameVersionChangeDiffType =
	| 'added'
	| 'removed'
	| 'updated'
	| 'modpack_unlinked'
	| 'game_version_updated'
	| 'loader_updated'
	| 'config_files_updated'

export interface GameVersionChangeVersion {
	id: string
	version_number: string
}

export interface GameVersionChangeEntry {
	type: GameVersionChangeDiffType
	id: Ulid | null
	file_name: string | null
	project_id: string | null
	project_title: string | null
	project_icon_url: string | null
	current_version: GameVersionChangeVersion | null
	new_version: GameVersionChangeVersion | null
}

export interface GameVersionPreviewResponse {
	new_game_version: string
	new_loader: LoaderId
	/** The provider turns this into "" — ContentDiffPreview.newLoaderVersion is not nullable. */
	new_loader_version: string | null
	has_unknown_content: boolean
	changes: GameVersionChangeEntry[]
}

export interface GameVersionChangeRequest {
	game_version: string
	loader: LoaderId | null
	loader_version: string | null
	incompatible_content: 'update_then_disable' | 'disable' | 'keep'
}

/* ======================= 8. Settings ======================= */

export interface ServerProperties {
	known: KnownProperties
	custom: Record<string, string>
	restart_required: boolean
}

export interface ServerPropertiesPatch {
	known?: Record<string, string | null>
	custom?: Record<string, string | null>
}

export type JreVendor = 'temurin' | 'corretto' | 'graal'

export interface StartupOptions {
	java_version: number | null
	jre_vendor: JreVendor | null
	java_path: string | null
	memory_mib: number
	memory_max_mib: number
	extra_flags: string[]
	/** Without the managed flags — those live in managed_flags (9.3). */
	startup_command: string
	original_invocation: string
	managed_flags: string[]
	/** Only in the response to the PATCH that dropped them; empty in the GET (9.3). */
	stripped_flags: string[]
	restart_required: boolean
}

export interface StartupOptionsPatch {
	java_version?: number | null
	jre_vendor?: JreVendor | null
	memory_mib?: number
	startup_command?: string | null
}

export interface JavaRuntime {
	major: number
	vendor: JreVendor
	version: string
	path: string | null
	source: 'system' | 'managed'
	/** false = known and obtainable, but not on disk. */
	installed: boolean
}

export interface JavaRuntimeList {
	runtimes: JavaRuntime[]
	default_major_for_game_version: number | null
}

export interface Allocation {
	port: number
	name: string
}

/** GET /allocations — bare list without the primary port (9.6). */
export type AllocationList = Allocation[]

export interface CreateAllocationRequest {
	name: string
	/** Panel admins only. */
	port?: number
}

export interface RenameAllocationRequest {
	name: string
}

export interface SetPrimaryResponse {
	primary_port: number
	allocations: Allocation[]
	restart_required: boolean
}

export interface LoaderInfo {
	id: LoaderId
	name: string
	kind: 'vanilla' | 'server' | 'modloader' | 'proxy'
	install_kind: 'download' | 'installer'
	has_loader_versions: boolean
	supports_properties: boolean
	supports_content: boolean
	source:
		| 'mojang'
		| 'papermc'
		| 'purpurmc'
		| 'leafmc'
		| 'fabricmc'
		| 'neoforged'
		| 'quiltmc'
		| 'minecraftforge'
	wave: 1 | 2
}

export interface LoaderList {
	loaders: LoaderInfo[]
}

export interface GameVersionEntry {
	version: string
	version_type: 'release' | 'snapshot'
}

export interface GameVersionList {
	loader: LoaderId
	/** Newest first. */
	game_versions: GameVersionEntry[]
	cached_until: Rfc3339
}

/**
 * Covers LoaderVersionEntry (installation-settings/types.ts:29-36), but is not
 * identical: their contract calls it `channelTag`, knows no null and no `released`.
 * Our side does the renaming.
 */
export interface LoaderBuild {
	id: string
	label: string
	stable: boolean
	channel_tag: 'ALPHA' | 'BETA' | null
	/** null for Purpur and Fabric — the sources keep no date per build there. */
	released: Rfc3339 | null
}

export interface LoaderBuildList {
	loader: LoaderId
	game_version: string
	/** Newest first, at most 500, the installed build always included. */
	builds: LoaderBuild[]
	truncated: boolean
	cached_until: Rfc3339
}

export type ContentPolicy = 'keep' | 'wipe_mods'

export interface InstallRequest {
	loader: LoaderId
	game_version: string
	/** null for Vanilla. */
	loader_version: string | null
	content_policy: ContentPolicy
}

export interface ResetRequest {
	loader: LoaderId
	game_version: string
	loader_version: string | null
	/** Fixed true; false is rejected with 400. */
	keep_backups: true
}

export interface InstallAccepted {
	operation: Operation
	warnings?: ServerWarning[]
}

export interface ResetToSetupResponse {
	server_id: Ulid
	flows: { intro: boolean }
}

/* ======================= 9. Backups ======================= */

export type BackupStatus = 'pending' | 'in_progress' | 'timed_out' | 'error' | 'done'
export type BackupOperationType = 'create' | 'restore'

export type BackupOperationState =
	| 'pending'
	| 'ongoing'
	| 'completed'
	| 'cancelled'
	| 'failed'
	| 'timed_out'

/** Identical to Archon.BackupsQueue.v1.BackupQueueOperation, except for operation_id. */
export interface BackupOperation {
	operation_type: BackupOperationType
	/** ULID; passed through in the adapter as `unknown as number` (10.1). */
	operation_id: Ulid
	state: BackupOperationState
	scheduled_for: Rfc3339
	started_at: Rfc3339 | null
	completed_at: Rfc3339 | null
	/** true only for the create of a safety backup out of a restore. */
	has_parent: boolean
	error: string | null
	/** = dismissed_at === null of the matching operation. */
	should_prompt: boolean
	/** Always false; we have no legacy baggage. */
	synthetic_legacy: false
	user_info: UserRef | null
}

/** Subset with state ∈ {pending, ongoing}. */
export interface BackupActiveOperation {
	backup_id: Ulid
	operation_type: BackupOperationType
	operation_id: Ulid
	has_parent: boolean
	scheduled_for: Rfc3339
	started_at: Rfc3339 | null
	synthetic_legacy: false
	user_info: UserRef | null
}

export interface Backup {
	id: Ulid
	name: string
	/** Moment of queueing, not of finishing. */
	created_at: Rfc3339
	/** State of the most recent operation of either kind — a restore too (10.1). */
	status: BackupStatus
	/** Always false; we know no locked backups. */
	locked: false
	automated: boolean
	/** 0 as long as status !== 'done'. Not part of Modrinth's type. */
	size_bytes: number
	/** Where the bytes lie — not the server's configured target (10.1, 22.10). */
	location: BackupLocation
	/** null for location === 'local'. Set by the sweeper (22.17). */
	drive_state: DriveFileState | null
	/** null for location === 'local'; otherwise the way to download it (22.19). */
	drive_web_link: string | null
	/** Newest first, at most 20. */
	history: BackupOperation[]
}

export interface BackupListResponse {
	active_operations: BackupActiveOperation[]
	backups: Backup[]
}

export interface CreateBackupRequest {
	/** 1–128 characters after trim. */
	name: string
}

export interface RenameBackupRequest {
	name: string
}

export interface RestoreBackupRequest {
	/** Name of the safety backup created beforehand. */
	name: string
}

export interface RestoreBackupResponse {
	restore_operation_id: Ulid
	safety_backup: { id: Ulid; create_operation_id: Ulid }
}

export interface RetryBackupResponse {
	operation_id: Ulid
	operation_type: BackupOperationType
}

export interface BulkDeleteBackupsRequest {
	backup_ids: Ulid[]
}

export interface BulkDeleteBackupsResponse {
	deleted: Ulid[]
	failed: Array<{ id: Ulid; error: string; message: string }>
}

export type BackupScheduleStatus =
	| 'completed'
	| 'failed'
	| 'timed_out'
	| 'skipped_unchanged'
	| 'skipped_limit'

export interface BackupSchedule {
	enabled: boolean
	/** 1–168. */
	interval_hours: number
	/** 0–23, only effective when interval_hours % 24 === 0. */
	hour_utc: number
	/** 1–50 and <= max_backups. */
	keep_last: number
	next_run_at: Rfc3339 | null
	last_run_at: Rfc3339 | null
	last_status: BackupScheduleStatus | null
	last_error: string | null
}

export type UpdateBackupScheduleRequest = Pick<
	BackupSchedule,
	'enabled' | 'interval_hours' | 'hour_utc' | 'keep_last'
>

/* ======================= 10. Per-server access ======================= */

export interface ServerMember {
	id: Ulid
	user: UserRef
	role: ServerRole
	permissions: PermissionMask
	/** null while the invitation is still open. */
	joined_at: Rfc3339 | null
	invited_at: Rfc3339
	last_invite_sent: Rfc3339 | null
	invite_resend_available_at: Rfc3339 | null
	pending: boolean
	is_owner: boolean
}

export interface ServerMemberList {
	members: ServerMember[]
}

export interface AddMemberRequest {
	user_id: Ulid
	role: Exclude<ServerRole, 'owner'>
}

export interface UpdateMemberRequest {
	role: Exclude<ServerRole, 'owner'>
}

/** Shape of Archon.ServerUsers.v1.ReinviteResponse. */
export interface ReinviteResponse {
	sent: boolean
	cooldown_seconds: number | null
	member: ServerMember
}

export interface Invitation {
	id: Ulid
	server: { id: Ulid; name: string }
	role: ServerRole
	invited_by: UserRef
	invited_at: Rfc3339
	last_invite_sent: Rfc3339 | null
}

export interface InvitationList {
	invitations: Invitation[]
}

/* --- Audit log: shape of Archon.Actions.v1.*, so parseAuditEvent runs unchanged --- */

export type AuditAction =
	| 'server_created'
	| 'server_reallocated'
	| 'server_repaired'
	| 'server_reset'
	| 'server_started'
	| 'server_stopped'
	| 'server_restarted'
	| 'server_killed'
	| 'console_cleared'
	| 'console_command_executed'
	| 'changed_server_name'
	| 'user_invited'
	| 'user_invite_revoked'
	| 'user_permission_modified'
	| 'user_removed'
	| 'addon_added'
	| 'addon_uploaded'
	| 'addon_disabled'
	| 'addon_enabled'
	| 'addon_deleted'
	| 'addon_updated'
	| 'modpack_changed'
	| 'modpack_unlinked'
	| 'port_allocation_added'
	| 'port_allocation_removed'
	| 'loader_version_edited'
	| 'game_version_edited'
	| 'server_properties_modified'
	| 'startup_command_modified'
	| 'java_runtime_modified'
	| 'java_version_modified'
	| 'file_uploaded'
	| 'file_deleted'
	| 'file_renamed'
	| 'file_edited'
	| 'backup_created'
	| 'backup_renamed'
	| 'backup_restored'
	| 'backup_deleted'

export interface AuditEntry {
	id: Ulid
	/** We never use type: 'support' — the icon for it is someone else's trademark. */
	actor: { type: 'user'; user_id: Ulid }
	action: { action: AuditAction; metadata: Record<string, unknown> | null }
	server_id: Ulid
	/** Always null: one world per server. */
	world_id: null
	timestamp: Rfc3339
}

export interface AuditLogPage {
	/** null on the last page. */
	next_offset: number | null
	data: AuditEntry[]
	users: Record<string, { username: string; avatar_url: string | null }>
	addons: Record<string, { title: string; slug: string | null; icon_url: string | null }>
	versions: Record<string, { name: string; version_number: string | null }>
}

export interface AuditLogQuery {
	limit?: number
	offset?: number
	order?: 'asc' | 'desc'
	min_datetime?: Rfc3339
	max_datetime?: Rfc3339
	actor?: Ulid[]
	action?: AuditAction[]
}

/* ======================= 11. Accounts and administration ======================= */

export interface LoginRequest {
	username: string
	password: string
}

export interface ChangePasswordRequest {
	current_password: string
	new_password: string
}

export interface UserSearchResponse {
	users: UserRef[]
}

export type SystemUserState = 'provisioning' | 'ready' | 'error'

export interface SystemUser {
	state: SystemUserState
	name: string
	uid: number | null
	error_message: string | null
}

export type CpuMode = 'cap' | 'share'

export interface UserLimits {
	memory_mib: number
	cpu_mode: CpuMode
	cpu_cores: number
	pids_max: number
	/** Disk across all servers and backups together; without a cgroup counterpart. */
	disk_mib: number
}

/** Every limit is null when no limit applies to this account (12.7). */
export interface MemoryUsage {
	limit_mib: number | null
	/** Sum of memory_mib over all own servers, running or not. */
	allocated_mib: number
	/** memory.current of the user cgroup. */
	used_bytes: number
}

export interface CpuUsage {
	limit_cores: number | null
	/** Averaged from cpu.stat over the measurement window; in share mode it can exceed limit_cores. */
	used_cores: number
}

export interface DiskUsage {
	limit_mib: number | null
	/** servers_bytes + backups_bytes, 60 s window. */
	used_bytes: number
	servers_bytes: number
	backups_bytes: number
	/**
	 * `false` when a directory was closed to the panel while counting (the game
	 * process is allowed to do that). The three numbers above are then a lower bound,
	 * and the interface says "at least" instead of naming a number it does not
	 * have (3.3).
	 */
	complete: boolean
}

export type LimitDimension = 'memory' | 'cpu' | 'pids' | 'disk'

export interface UserUsage {
	memory: MemoryUsage
	cpu: CpuUsage
	pids: { limit: number | null; used: number }
	disk: DiskUsage
	servers: { total: number; running: number }
	/** true as soon as allocated_mib > limit_mib or the disk is fuller than allowed. */
	over_limit: boolean
	over_limit_dimensions: LimitDimension[]
	measured_at: Rfc3339
}

/** system_user_not_ready beats over_limit — without a system user nothing works at all. */
export type BlockedReason = 'over_limit' | 'system_user_not_ready' | null

export interface Capabilities {
	can_create_servers: boolean
	can_start_servers: boolean
	can_manage_panel_users: boolean
	blocked_reason: BlockedReason
}

export interface PanelUser {
	id: Ulid
	username: string
	avatar_url: string | null
	panel_role: PanelRole
	/** The account's address, lowercased; null for accounts created by hand. Always a
	 *  usable address — there is no "confirmed yes/no" field (3.3). */
	email: string | null
	/** How the account came about; 'registration' means someone signed themselves up (20). */
	origin: AccountOrigin
	created_at: Rfc3339
	last_login_at: Rfc3339 | null
	must_change_password: boolean
	system_user: SystemUser
	/** null when no limit applies — then there is no form either (12.7). */
	limits: UserLimits | null
	usage: UserUsage
}

export interface Me extends PanelUser {
	capabilities: Capabilities
	session: { id: Ulid; expires_at: Rfc3339 }
}

export interface OwnedServerRef {
	id: Ulid
	name: string
	memory_mib: number
	running: boolean
}

export interface AdminUserDetail extends PanelUser {
	owned_servers: OwnedServerRef[]
	active_sessions: number
}

export interface AdminUserList {
	users: PanelUser[]
	total: number
}

export interface CreateUserRequest {
	username: string
	password: string
	panel_role: PanelRole
	/** Without an address this account has no "forgot password" (12.3, 21.7). */
	email?: string
	must_change_password?: boolean
	limits?: UserLimits
}

export interface UpdateUserRequest {
	username?: string
	panel_role?: PanelRole
	password?: string
	/** null deletes the address and with it the path through 21.1. */
	email?: string | null
	must_change_password?: boolean
}

export type DeleteUserServers = 'delete' | 'transfer'

export interface UserLimitsResponse {
	/** null for an account without limits; then there is no form (12.7). */
	limits: UserLimits | null
	usage: UserUsage
	host: { cpu_cores: number; assignable_memory_mib: number; assignable_disk_mib: number }
}

export interface HostCapacity {
	cpu_cores: number
	memory_total_bytes: number
	reserved_memory_mib: number
	assignable_memory_mib: number
	/** statvfs of the data directory, machine-wide. */
	disk_total_bytes: number
	assignable_disk_mib: number
	/** Sum of the user limits, not of the -Xmx; panel admins are not part of it. */
	allocated: { memory_mib: number; cpu_cores: number; disk_mib: number }
	used: { memory_bytes: number; cpu_cores: number; pids: number }
	user_count: number
	/** Accounts missing from allocated because nothing was promised to them. */
	unlimited_users: number
	default_limits: UserLimits
	measured_at: Rfc3339
}

export interface PanelSettings {
	/** Becomes ServerNet.ip; may be a name. */
	public_address: string | null
	port_pool: { from: number; to: number }
	default_limits: UserLimits
	max_upload_bytes: number
	max_backups_per_server: number
	/** mclo.gs and api.modrinth.com — and the Drive endpoints (22), but not mail delivery
	 *  (12.10). */
	external_services_enabled: boolean
	max_concurrent_operations: number
	stop_grace_seconds: number
	/** The switch itself. What the sign-in page offers is stated by AuthOptions (20.1). */
	registration_enabled: boolean
	/** Only takes effect when the first one is on. Default true. */
	registration_requires_approval: boolean
}

/* ======================= 12. WebSocket, server → client ======================= */

export interface WsServerMessage {
	type: 'server'
	server: Server
}

export interface WsStateMessage {
	type: 'state'
	power_state: PowerState
	target: PowerTarget | null
	uptime_seconds: number
	exit_code: number | null
	oom_killed: boolean
}

export interface WsStatsMessage {
	type: 'stats'
	cpu_percent: number
	ram_usage_bytes: number
	ram_total_bytes: number
	storage_usage_bytes: number
	storage_total_bytes: number
}

export interface WsOperationsMessage {
	type: 'operations'
	revision: number
	busy_reasons: BusyReasonCode[]
	operations: Operation[]
}

export interface WsConsoleHistoryStartMessage {
	type: 'console_history_start'
	total_lines: number
	/** > 0 means: the history is incomplete. */
	dropped_lines: number
}

export interface WsConsoleMessage {
	type: 'console'
	/** Sequence number of the first line; per server, never reset. */
	seq: number
	lines: string[]
}

export interface WsConsoleHistoryEndMessage {
	type: 'console_history_end'
}

export interface WsConsoleClearedMessage {
	type: 'console_cleared'
}

export interface WsContentChangedMessage {
	type: 'content_changed'
	reason: 'updates_checked' | 'external_change'
}

export interface WsBackupListChangedMessage {
	type: 'backup_list_changed'
}

export interface WsStartupChangedMessage {
	type: 'startup_changed'
	java_version: number | null
	jre_vendor: JreVendor | null
	memory_mib: number
	startup_command: string
	original_invocation: string
	restart_required: boolean
}

export interface WsNetworkChangedMessage {
	type: 'network_changed'
	primary_port: number
	allocations: Allocation[]
}

export type WsMessage =
	| WsServerMessage
	| WsStateMessage
	| WsStatsMessage
	| WsOperationsMessage
	| WsConsoleHistoryStartMessage
	| WsConsoleMessage
	| WsConsoleHistoryEndMessage
	| WsConsoleClearedMessage
	| WsContentChangedMessage
	| WsBackupListChangedMessage
	| WsStartupChangedMessage
	| WsNetworkChangedMessage

/** There is no message from client to server (13.1). */
export type WsClientMessage = never

/* ======================= 13. playit.gg (section 18) ======================= */
/* These shapes live today in `web/src/api/playit.ts` and not in `types.ts`, because the eleven
   calls sit there next to their rules until `client.ts` knows them. The contract is the same. */

export type PlayitAgentState = 'absent' | 'starting' | 'running' | 'failed'
export type PlayitBinaryState = 'absent' | 'fetching' | 'ready' | 'failed'
export type PlayitAccountStatus = 'guest' | 'email_not_verified' | 'verified'
export type PlayitClaimState = 'waiting_for_visit' | 'waiting_for_user' | 'accepted' | 'rejected'
export type PlayitTunnelState = 'none' | 'pending' | 'online' | 'offline' | 'missing' | 'failed'
/** Verbatim from playit's `ConnectAddress`. */
export type PlayitAddressKind = 'auto' | 'ip4' | 'ip6' | 'addr4' | 'addr6' | 'domain'
/** 18.5 and 18.11: what happens to the tunnels on disconnect. */
export type PlayitTunnelDisposal = 'delete' | 'keep'

export interface PlayitAgent {
	state: PlayitAgentState
	version: string | null
	detail: string | null
}

/** Four ports per **user account**, sixteen with Premium. */
export interface PlayitPorts {
	used: number
	limit: number
	/** Of those, on servers that belong to someone else (only after the takeover, PLAYIT.md 7). */
	for_others: number
}

export interface PlayitClaim {
	/** Ten hex characters. */
	code: string
	/** Always https://playit.gg/claim/<code> — append nothing. */
	url: string
	state: PlayitClaimState
	started_at: Rfc3339
	/** Our own deadline, not playit's. */
	expires_at: Rfc3339
}

export interface PlayitStatus {
	configured: boolean
	agent_id: string | null
	account_status: PlayitAccountStatus | null
	is_self_managed: boolean
	has_premium: boolean
	agent: PlayitAgent
	binary: {
		state: PlayitBinaryState
		version: string | null
		arch: string
		detail: string | null
	}
	ports: PlayitPorts
	claim: PlayitClaim | null
	last_error: string | null
	checked_at: Rfc3339 | null
}

/** 18.10. Without `claim` and without any field that could carry the key. */
export interface PlayitOverview {
	user_id: Ulid
	username: string | null
	configured: boolean
	account_status: PlayitAccountStatus | null
	is_self_managed: boolean
	has_premium: boolean
	agent: PlayitAgent
	ports: PlayitPorts
	last_error: string | null
	checked_at: Rfc3339 | null
}

export interface PlayitAddress {
	address: string
	kind: PlayitAddressKind
}

/** 18.7. `state: "none"` is a server without an address. Carries no port numbers. */
export interface ServerTunnel {
	state: PlayitTunnelState
	addresses: PlayitAddress[]
	local_port: number | null
	detail: string | null
	created_at: Rfc3339 | null
	checked_at: Rfc3339 | null
}

/* ======================= 14. Mail delivery (section 19) ======================= */
/* Like the playit shapes, these live in a module of their own (`web/src/api/mail.ts`),
   so that `client.ts` is not touched by every area. The contract is the same. */

export type MailProvider = 'resend'
/** 'file_sink' is CRAFTPANEL_MAIL_SINK: every mail becomes a file, nothing goes out (19.2). */
export type MailState = 'not_configured' | 'configured' | 'file_sink'
/** The eight templates from 19.12. At the same time the path value of 19.9. */
export type MailKind =
	| 'verify_email'
	| 'address_already_registered'
	| 'account_awaiting_review'
	| 'account_approved'
	| 'account_rejected'
	| 'reset_password'
	| 'password_changed'
	| 'test'
export type MailDeliveryState = 'queued' | 'sending' | 'sent' | 'failed'

/** Never carries one character of the key; `key_set_at` is all that stands here of it. */
export interface MailSettings {
	provider: MailProvider
	state: MailState
	key_set_at: Rfc3339 | null
	from_address: string
	from_name: string
	reply_to: string | null
	/** The panel's address with scheme. Not public_address (19.2). */
	link_base: string | null
	/** What a link built from it looks like; null when link_base is missing. */
	example_link: string | null
	/** Set only for state === 'file_sink'. */
	sink_path: string | null
	/** 0 = no daily brake of our own. */
	daily_limit: number
	sent_today: number
	queued: number
	failed: number
	last_test_at: Rfc3339 | null
	last_error: string | null
	last_error_at: Rfc3339 | null
}

export interface UpdateMailSettingsRequest {
	from_address: string
	from_name: string
	reply_to: string | null
	link_base: string | null
	daily_limit: number
	/** Omit or null = unchanged, '' = delete, text = replace (19.3). */
	api_key?: string | null
}

export interface SendTestMailRequest {
	/** Omit = your own address from 3.3. */
	to?: string
}

export interface SendTestMailResponse {
	/** Resend's mail id. */
	id: string
	to: string
}

export interface MailOutboxEntry {
	id: Ulid
	kind: MailKind
	to_address: string
	subject: string
	state: MailDeliveryState
	attempts: number
	next_attempt_at: Rfc3339 | null
	provider_id: string | null
	last_error: string | null
	/** false as soon as the body was emptied after delivery (19.7). */
	has_content: boolean
	created_at: Rfc3339
	sent_at: Rfc3339 | null
}

export interface MailOutboxList {
	mails: MailOutboxEntry[]
	total: number
}

/* ======================= 15. Accounts and sign-up (section 20) ======================= */

export type AccountOrigin = 'admin' | 'registration'
export type RegistrationState = 'email_unverified' | 'awaiting_approval'

/** The only endpoint a sign-in page asks without a session (20.1). */
export interface AuthOptions {
	/** Already the conjunction of the switch (12.10) and mail readiness (19.2). */
	registration_enabled: boolean
	registration_requires_approval: boolean
	password_reset_enabled: boolean
}

export interface RegisterRequest {
	username: string
	email: string
	password: string
}

/** Byte for byte the same response for new, known and blocked addresses (20.2). */
export interface RegisterResponse {
	status: 'check_your_email'
}

export interface VerifyEmailRequest {
	token: string
}

export interface VerifyEmailResponse {
	state: 'active' | 'awaiting_approval'
}

export interface ResendVerificationRequest {
	email: string
}

export interface Registration {
	id: Ulid
	username: string
	email: string
	state: RegistrationState
	/** Only in the admin list; disappears with the approval (20.5, 20.13). */
	signup_ip: string | null
	created_at: Rfc3339
	verified_at: Rfc3339 | null
}

export interface RegistrationList {
	registrations: Registration[]
	total: number
}

export interface RejectRegistrationRequest {
	/** Stays in the panel; the mail does not name it (20.7). */
	reason?: string
}

/* ======================= 16. Forgot password (section 21) ======================= */

export interface PasswordResetRequest {
	email: string
}

export interface VerifyPasswordResetRequest {
	/** In the body, never in the URL (1.2, 21.5). */
	token: string
}

export interface VerifyPasswordResetResponse {
	username: string
}

export interface ConfirmPasswordResetRequest {
	token: string
	new_password: string
}

/* ======================= 17. Google Drive (section 22) ======================= */

/**
 * The state of a **connection** (22.3). For "never connected" and "connecting right now" this
 * list deliberately has no word: there `state` is simply `null`, what is running right now stands
 * in `DriveStatus.link`, and why the last attempt failed in `last_error`.
 */
export type DriveAccountState = 'connected' | 'revoked' | 'error'
export type DriveLinkState = 'waiting' | 'accepted' | 'denied' | 'expired'
export type DriveFileState = 'present' | 'missing' | 'trashed' | 'unreachable'
export type BackupLocation = 'local' | 'drive'
export type BackupTargetPolicy = 'user_choice' | 'drive_only' | 'local_only'
/** 22.7: what happens to the files on disconnect. 22.14 knows only 'keep'. */
export type DriveFileDisposal = 'delete' | 'keep'
/**
 * Why a target is not selectable (22.9). `not_connected` also covers revoked access;
 * `policy` with `effective_target: 'drive'` is the *healthy* state of a `drive_only` panel, and only
 * `not_configured` and `not_connected` are the two that 10.2 answers with a refusal.
 */
export type BackupTargetReason = 'ok' | 'not_configured' | 'not_connected' | 'policy'

export interface DriveLink {
	/** A secret of its owner. Never in an admin response, never in a log (22.11). */
	user_code: string
	/** Google's own field name (not verification_uri); always https://www.google.com/device. */
	verification_url: string
	state: DriveLinkState
	started_at: Rfc3339
	/** Google's expires_in, not our deadline. */
	expires_at: Rfc3339
	/** Seconds between two polls, from Google's interval. */
	interval: number
}

export interface DriveStatus {
	/** Has the operator entered a Google project (22.2)? */
	panel_configured: boolean
	/** Is there a token for this account? Does not say that it still holds (22.3). */
	configured: boolean
	/** null = nothing connected: never was, busy right now, or the last attempt failed. */
	state: DriveAccountState | null
	google_name: string | null
	google_email: string | null
	folder_name: string
	/** null = unlimited (Workspace). */
	storage_limit_bytes: number | null
	storage_usage_bytes: number | null
	/** Only your own running operation. */
	link: DriveLink | null
	/** A sentence, never an identifier: the reason for a failed operation too (22.5). */
	last_error: string | null
	checked_at: Rfc3339 | null
}

/** 22.11. Without `link` and without any field that could carry a secret. */
export interface DriveOverview {
	user_id: Ulid
	username: string
	/** null = nothing connected (mid-operation or a failed one), never 'error' (22.11). */
	state: DriveAccountState | null
	google_email: string | null
	storage_limit_bytes: number | null
	storage_usage_bytes: number | null
	/** How many backups of this account lie in the Drive, and how large they are. */
	backups: number
	backup_bytes: number
	last_error: string | null
	checked_at: Rfc3339 | null
}

export interface DriveAdminOverview {
	configured: boolean
	/** Not a secret; the secret is a file (22.12). */
	client_id: string | null
	target_policy: BackupTargetPolicy
	folder_name: string
	accounts: DriveOverview[]
}

export interface UpdateDriveSettingsRequest {
	client_id: string | null
	/** Omit or null = unchanged, '' = delete, text = replace (22.12). */
	client_secret?: string | null
	target_policy: BackupTargetPolicy
	folder_name: string
}

export interface BackupTarget {
	target: BackupLocation
	/** What the next run really takes. */
	effective_target: BackupLocation
	policy: BackupTargetPolicy
	reason: BackupTargetReason
}

export interface UpdateBackupTargetRequest {
	target: BackupLocation
}
```

---

## 15. Coverage of the provider contracts

The evidence that this contract serves the interface completely. The line numbers in column 1 are
the declaration.

### 15.1 `ModrinthServerContext` — 22 members, one by one

Declaration `providers/server-context.ts:37-72`. Injected without a fallback
(`create-context.ts:66` throws) — the field `currentUserPermissions` alone is therefore required
for **every** shared layout, not only for the access page.

| # | Field | Where the value comes from |
|---|---|---|
| 1 | `serverId` `:38` | route parameter; identical with `Server.id` |
| 2 | `worldId` `:39` | **constant `ref("default")`** (1.9) |
| 3 | `server` `:40` | 4.3, then live over WS `server`. The adapter builds `Archon.Servers.v0.Server` from it: `server_id` ← `id`, `mc_version` ← `game_version`, `loader` ← display name from 9.11, `current_user_permissions` ← mask with `as unknown as number`; `suspension_reason: null`, `datacenter: ""`, `notices: []`, `node: null`, `is_medal: false`, `sftp_*: ""` as constants |
| 4 | `serverFull` `:41` | **constant `null`.** Of nine fields we would have three; checked: no reader in `layouts/shared/` or `components/servers/`. The single occurrence `composables/server-backup.ts:14` pulls its list from a query of its own |
| 5 | `currentUserPermissions` `:42` | `Server.current_user_permissions` (1.10) |
| 6 | `isConnected` `:45` | provider: `true` on `open`, `false` on `close` |
| 7 | `isWsAuthIncorrect` `:46` | provider: `true` on close code `4401`/`4403` (13.6) |
| 8 | `powerState` `:47` | WS `state.power_state` |
| 9 | `powerStateDetails` `:48` | WS `state.exit_code`/`oom_killed`, only on `crashed`, otherwise `undefined` |
| 10 | `isServerRunning` `:49` | provider: `powerState === 'running'` |
| 11 | `stats` `:50` | WS `stats`, worked out into `{current, past, graph}` in the provider (13.4) |
| 12 | `uptimeSeconds` `:51` | WS `state.uptime_seconds`, counted up locally in between |
| 13 | `isSyncingContent` `:54` | `busy_reasons.includes("syncing_content")` |
| 14 | `busyReasons` `:57` | `busy_reasons` from WS `operations`, turned into `{reason: MessageDescriptor}` through `BUSY_MESSAGE` — **not a flat `{id}`**, the interface reads `r.reason.id` and hands `r.reason` to `formatMessage` |
| 15 | `fsAuth` `:60` | **gap, deliberate: `ref(null)`.** Modrinth talks to a second service and needs a token of its own; we have one process and one cookie. No layout reads the field — the only readers are `server-manage-core-runtime.ts:385,417`, and we are replacing that one (15.6) |
| 16 | `fsOps` `:61` | `ref([])`. Outside the runtime only `wrapped/` code touches it |
| 17 | `fsQueuedOps` `:62` | `ref([])`, same |
| 18 | `refreshFsAuth` `:63` | empty `async` function |
| 19 | `uploadState` `:66` | purely client-side, from the XHR progress (7.8) |
| 20 | `cancelUpload` `:67` | purely client-side, `xhr.abort()` plus emptying the queue |
| 21 | `activeOperations` `:70` | projection of `operations` onto `FileOperation`, see 15.2 |
| 22 | `dismissOperation` `:71` | `(opId, action)` → 5.5 or 5.4 |

**Two building blocks fetch their token past this contract** and still need a replacement:
`composables/use-server-image.ts:67-78` and `components/servers/edit-server-icon/EditServerIcon.vue:153-234`
load and write `/server-icon.png` over `kyros.files_v0`. Both go through the client adapter
(9.18) onto 7.7 and 7.8.

### 15.2 `FileOperation` — the target shape of `activeOperations`

Source `files-tab/types.ts:32-41`. **Only** operations of the kind `unarchive` are projected here:
`FileOperationAdmonition` is cut for extraction — the heading is fixed at
"Extracting {source}" (`:68-79`) and the icon is `PackageOpenIcon` (`:11`). A backup operation in
this list would read "Extracting".

```ts
const toFileOperation = (op: Operation) => ({
  id: op.id,                                  // no id, no cancel (:26) and no dismiss
  op: op.kind,                                // only part of the stack key
  src: op.src ?? '',                          // required: :96 calls props.op.src.includes() unchecked
  state: op.state,                            // verbatim; 'failed'.startsWith('fail') === true
  progress: op.progress,                      // 0…1
  bytes_processed: op.bytes_processed ?? undefined,
  files_processed: op.files_processed ?? undefined,
  current_file: op.current_file ?? undefined,
})

const activeOperations = computed(() =>
  operations.value.filter((op) => op.kind === 'unarchive' && op.state !== 'cancelled')
    .map(toFileOperation),
)
```

**An observed defect of the foreign component that we work around:** `isTerminal` (`:94`) knows
only `done` and `fail*`; a `cancelled` would count as running there, would keep showing a cancel
button and could not be dismissed. Hence the filter.

**What the contract does not take up: `cancellable`.** `FileOperation` has no such field, and
`:26-37` shows the cancel button on **every** unfinished operation with an `id`. An `unarchive`
past `applied_at` therefore sits in the list with `cancellable: false` and still has a clickable
button in front of it; the `409 operation_not_cancellable` lands in `dismissOperation`, which only
writes the error to the console. Decision: **`unarchive` stays cancellable to the end** — past
`applied_at`, canceling means the entries already moved stay where they are (5.4).

### 15.3 `FileManagerContext`

Source `files-tab/providers/file-manager.ts:13-67`. `items` ← 7.3 **assigned unchanged**
(`ApiFileItem` is identical with `FileItem`); `loading`/`error` from the same request;
`currentPath`, `navigateTo`, `editingFile`, `startEditing`, `stopEditing`, `refresh` are provider
state; `createItem` ← 7.4, `renameItem`/`moveItem` ← 7.5, `deleteItem` ← 7.6,
`readFile`/`readFileAsBlob`/`downloadFile` ← 7.7, `writeFile`/`uploadFiles` ← 7.8, `extractFile`
← 7.9, `prefetchDirectory`/`prefetchFile` ← 7.3/7.7 into the cache, `basePath` ←
`FilesMetaResponse.root_path`, `activeOperations`/`dismissOperation` as in 15.1,
`isBusy` = `busyReasons.length > 0 || !canWriteFiles`, `busyTooltip`/`busyWarning` from the first
reason. Not set: `showInstallFromUrl`, `openInFolder`, `downloadButtonLabel`, `uploadingLabel`,
`canRestart`, `restartServer`, `canShareToMclogs` (7.10).

`startEditing` stores the path **without** a leading `/`: `FileNavbar.vue:404-410` decides on
"Share to mclo.gs" with `startsWith('logs')` and `startsWith('crash-reports')` — with
`/logs/latest.log` both conditions are dead. The editor normalizes back to `/` on read and write
anyway.

### 15.4 `ConsoleManagerContext`

Source `console/providers/console-manager.ts:8-34`. `logLines` ← WS `console*` or 6.5;
`logSources` = an invented live source plus 6.4; `activeLogSourceIndex` is pure client state and
**must be writable** (the layout writes into it, `:31`); `sendCommand` ← 6.1; `showCommandInput` =
`true`; `disableCommandInput` = `!canExecuteCommands || power_state !== 'running'`; `onClear` ←
6.2; `clearDisabled` = `!canExecuteCommands`; `onDelete` ← 6.6; `deleteDisabled` =
`!canWriteFiles` or "`latest.log` and the server is running"; `shareDisabled` = `!isConnected` or
outgoing services off; `emptyStateType` = `'server'`; `crashAnalysis` ← 6.3; `onDismissCrash` =
local plus a marker in `localStorage` for 30 minutes.

Two traps you only find by reading: **`logLines` has to be a `shallowRef`** that is appended to in
place, with `triggerRef` called afterwards. The layout watches with `watch(ctx.logLines, …)`
**without `deep`** (`:328`); a `ref([])` plus `push` triggers **zero** runs (the console would stay
empty), and an array replaced per batch redraws the **whole** terminal every time, over
`lines !== oldLines` (`:343-347`). And `deleteDisabledTooltip` is to be handed through as a **plain
string**, not as a `Ref` — unlike every other tooltip (`:52`). The tooltip
`disableCommandInputTooltip` must **not** be set for "the server is not running", because the
layout derives the placeholder text from it (`:239-241`).

### 15.5 `ContentManagerContext` and `InstallationSettingsContext`

`content-tab/providers/content-manager.ts`: `items`/`modpack`/`contentTypeLabel`/`permissions`
← 8.1, `toggleEnabled` ← 8.3/8.4, `deleteItem`/`bulkDeleteItems` ← 8.5, `bulkEnableItems`/
`bulkDisableItems` ← 8.3/8.4, `refresh` ← 8.1 with `refresh_updates=true`, `uploadFiles` ← 8.8,
`getDeleteDependencyWarning` ← 8.9, `bulkUpdateAll`/`bulkUpdateItems`/`updateItem`/`switchVersion`
← 8.6, `updateModpack` ← 8.11, `viewModpackContent` ← 8.2, `unlinkModpack` ← 8.12,
`isBusy`/`busyMessage` from `busyReasons`, `hasUpdateSupport` = `true` (no ref!), `getItemId` =
`item => item.id`, `deletionContext` = `'server'`, `mapToTableItem` a pure reshaping (8.1) — with
`projectLink` and `hideSwitchVersion` on `project_id`, not on the cached `project`.
**`isPackLocked` is a required field with no effect** — read in two places, evaluated in none;
we supply `ref(false)`. **We do not supply `bulkUpdateItem`**: `layout.vue:743` picks it only when
neither `bulkUpdateAll` nor `bulkUpdateItems` is set — it would be dead code.
`getDeleteWarning`/`getDisableWarning` return `null` (they exist for Modrinth's managed
instances), `browse` is a router jump, `skipNonEssentialWarnings` and `filterPersistKey` are
local.

`installation-settings/providers/installation-settings.ts`: `installationInfo`/`currentPlatform`/
`currentGameVersion`/`currentLoaderVersion` from `Server`; `availablePlatforms` ← 9.11;
`resolveGameVersions`/`resolveHasSnapshots` ← 9.12, `resolveLoaderVersions` ← 9.13; `save` and
`saveWithoutAutoFix` ← 9.14; `repair` ← 9.15; `isLinked`/`modpack`/`reinstallModpack`/
`swapModpack`/`unlinkModpack`/`previewSave`/`disableAllContent`/`disableIncompatibleContent`/
`updaterModalProps` ← section 8; `isServer` = `true`, `isApp` = `false`, `lockPlatform` =
`false`, `isManagedModpack` = `false`.

**The three `resolve*` are synchronous** — their signatures return arrays, not promises, and they
are called inside `computed` and in the middle of rendering. They must therefore **fetch nothing**
and only read an already filled, reactive cache; that is exactly what `editingPlatformRef` and
`editingGameVersionRef` are in the contract for — the queries hang off them.

The layout renders a row with `value: null` in `installationInfo` as a **loading bar**, not as an
empty value, so on Vanilla the build row is to be left out, not emptied. And besides the contract
the layout has an **emit** `reset-server` (`:344`) that our page has to handle: it leads to 9.16,
but `cancelEditing` runs first, so the selection is already reset.

### 15.6 What we write ourselves instead of borrowing

`composables/server-manage-core-runtime.ts` (450 lines) sits in `composables/`, but is bound hard
to Modrinth's client: `client.archon.sockets.safeConnect` (`:321`),
`client.kyros.files_v0.modifyOperation` (`:373`), `client.archon.servers_v0.getFilesystemAuth`
(`:385`). We write `provideModrinthServerContext` ourselves — that is no surprise, the plan
budgets 200 to 400 lines per area, but it means these 450 lines belong to our work and not to the
borrowed library.

`components/servers/admonitions/ServerPanelAdmonitions.vue` **we borrow unchanged**: it does
inject Modrinth's client and call `backups_queue_v1.ackCreate`, `ackRestore`, `cancelCreate`,
`cancelRestore`, `retry` and `backups_v1.delete`, but those six calls are exactly what the
adapter from 9.18 serves onto 5.4, 5.5 and 10.7. That keeps all four displays foreign
(`InstallingBanner`, `BackupAdmonition`, `FileOperationAdmonition`, `UploadAdmonition`) and saves
us rebuilding about 120 lines together with their stacking and precedence rules.

The **filter bar of the audit log**, by contrast, is our own work: `AuditLogTable` renders filters
only when the `#filters` slot is filled (`:15-18`), and Modrinth's filling sits under `wrapped/` —
about 500 lines including one translation per action name. Without it, `actor` and `action` from
11.9 have no controls.

Also required, or the injection alone throws: `injectNotificationManager` (files tab, editor,
console), `injectModrinthClient` (console, editor, access page, all four settings pages),
`injectModalBehavior` (console). `injectPageContext` and `injectFilePicker` have fallbacks and may
be missing.

---

## 16. The contradiction list

Fifty resolved contradictions between the eight area contracts. Short forms: `A` =
creation-and-progress, `Au` = auth, `D` = files, `E` = settings, `I` = content,
`K` = console, `Se` = servers, `Si` = backups.

### 16.1 The progress model

| # | Contradiction | Decision |
|---|---|---|
| 1 | Five progress models: `operations` (A), `file_ops` (D), `install_state` (E), `backup_progress` (Si), `content_task` (I) | one — `Operation` plus WS `operations`; `A` is the default, the other four go |
| 2 | Three sets of operation endpoints: `…/files/operations[/cancel|dismiss]` (D), `…/backup-operations/:id/{ack,cancel}` (Si), `…/content/tasks/:id` (I) | one under `…/operations` (5.1–5.7); the adapters map the foreign signatures onto it |
| 3 | Phases in triplicate: `resolving/downloading/installing/modpack/addons` (Se), `analyzing/installing_loader/installing_pack/addons` (A), `resolve/download/verify/run_installer/write_config/done` (E) | one list with seven values (5.9) that covers the `verify` and `run_installer` of the second wave; the mapping onto Modrinth's four happens in the provider |
| 4 | Error states `failed-path`/`failed-corrupt`/`failed-io` (D) against `failed` plus `error.code` (A) | `failed` plus code — it satisfies `startsWith('fail')` just as well and costs no state names |
| 5 | Never send `cancelled` (D) against send it and filter in the provider (A) | send, filter (15.2) |
| 6 | Retention of finished operations: 10 minutes (D) against 7 days (A) | 7 days |
| 7 | Cancel answers `204` (D) against `200` with the operation (A) | `200` with the operation on cancel, `204` on dismiss |
| 8 | `revision` as race protection exists only in D, not in A | taken over for **all** operation snapshots (5.2) — the race is there just the same |
| 9 | Extracting locks every write path (D 2.4) against `unarchive` sets no lock reason (A 4.6) | no lock reason and no `409` — otherwise the file manager locks itself out; the race on a rename at the same time is named and accepted (5.8) |
| 10 | `busy_reasons` comes from the server (A) against "no `busy_reasons` in the API, the client works it out" (Se 1.5) | from the server: it enforces the lock with `409`, otherwise the grayed-out button and the refusal drift apart |
| 11 | Two 5-second polls: the server list (Se) and `GET /operations` (A) | one — `GET /operations?state=active`, the list reloads on terminal states (4.1) |
| 12 | Backups: `should_prompt` plus `ack` (Si) against `dismissed_at` plus `dismiss` (A) | the same notion, one mechanism: `should_prompt === (dismissed_at === null)`, `ack` is `dismiss` |
| 13 | `timed_out` "we do not produce it" (A) against a timeout after 10 minutes (Si) | `failed` with `error.code = "timeout"`; the backup adapter maps that onto `timed_out` |
| 14 | `backup_progress` as a WS message of its own (Si) | it goes; `handleWsBackupProgress` is fed from `operations` (13.5). `backup_list_changed` stays, because renaming, deleting and cleaning up are not operations |
| 15 | Rebuild `ServerPanelAdmonitions` (A 5.3) against unchanged with a client front (Si 1.1) | unchanged; the front is there for four more components anyway (15.6) |
| 16 | Backup `retry` twice: generic (A) and `…/backups/:id/retry` (Si) | the backup-specific one stays, because the vendor method passes the backup id; 5.6 excludes `backup_*` |
| 17 | `install_java` with no decided lock reason (A 4.6) | `installing` — a server without its runtime does not start anyway, and an operation without a lock reason that still triggers `409` would be exactly the divergence from #10 |

### 16.2 Endpoints and paths

| # | Contradiction | Decision |
|---|---|---|
| 18 | `POST /servers` answers `201` with a full `Server` (Se) against `201` with `{server_id, operation}` (A) | `201` with **both**: `{server, operation}`. The worry about two schemas goes away, because this document has only one |
| 19 | `POST /servers` field names `source`/`mc_version`/`ram_mib`/`accept_eula` (Se) against `content`/`game_version`/`memory_mib`/`eula_accepted` (A) | the second form — it lines up with `E` and `I`, the first only with itself |
| 20 | `.mrpack` as `multipart` to `POST /servers` (Se) against `PUT …/operations/:op_id/payload` (A) | `payload` — one identifier less, and the cleanup rule applies without an addition |
| 21 | `DELETE /servers/:id` in triplicate: `204` at once (Se), `202` with the operation (A), `202` with `{state:"deleting", backups_kept}` (E) | `202` with the operation plus `?keep_backups` (4.5); deleting a directory of 40 GiB is an operation, not the span of one response |
| 22 | `GET /loaders` with two different bodies (Se, E) and two version shapes (`…/versions` nested against `…/game-versions` + `…/builds`) | `E`'s catalog and the two-step shape — the resolvers of the settings page are synchronous and need it; the create wizard builds its manifest from it in the frontend |
| 23 | `POST …/content/modpack/repair` (I) against `POST …/repair` (E) | `E`; `I` had already dropped its own endpoint itself |
| 24 | `update_channel` is read (I 2.1) but has a write path nowhere | `PATCH /servers/:id` (4.4) — no new path for one field |
| 25 | Path collision `…/backups/schedule` against `…/backups/:backup_id` | the ULID pattern in the router, not the order of registration (1.3) |
| 26 | Two `modrinth` prefixes: `/api/v1/modrinth/*` (forwarding, I) and `/modrinth/v0/backups/…` (alias, Si) | both stay, but they are different prefixes; the alias is the **only** exception to `/api/v1/` and has no state of its own |
| 27 | Four pagination forms (`limit/offset`, `before`, `after`, none at all) | three, each with a reason, plus hard upper limits instead of silent truncation (1.8) |
| 28 | Operation width, the port pool, the "outgoing services" switch and the upload limit are presumed by A, E, K and D, but defined by nobody | one new endpoint pair `GET/PUT /admin/settings` (12.10) |

### 16.3 Names and units

| # | Contradiction | Decision |
|---|---|---|
| 29 | `current_user_permissions` as an array (Se) against a string (Au) | string — checked: `parsePermissions` calls `value.split('|')` on everything that is not a number; an array would run into a `TypeError` |
| 30 | Permissions as `files:read`/`files:write` (D) against bit names (Au) | bit names; names of our own are impossible anyway, `parsePermissionString` discards the unknown silently |
| 31 | `mc_version` (Se, E) against `game_version` (A, I, E) | `game_version` on the wire; the adapter, which fills eleven fields with constants anyway, sets `mc_version` from it |
| 32 | `Server.server_id` (Se) against `id` everywhere else | `id` for a resource's own identifier, `<resource>_id` for references (1.3) |
| 33 | `ram_mib` (Se) against `memory_mib` (A, E) against `memory_bytes` (Au) | rule: allocated `_mib`, measured `_bytes` (1.5) |
| 34 | `created`/`last_login` (Au, Modrinth's form) against `created_at` everywhere else | `created_at`/`last_login_at`; the optional `AuthProvider` renames in three lines |
| 35 | `ServerOwner` (Se), `BackupUserInfo` (Si) and `UserRef` (Au) are the same object under three names | `UserRef` |
| 36 | `loader` in display spelling `"Paper"` (Se) against lowercase (E, I) | lowercase; `LoaderInfo.name` supplies the display name, because `formatLoaderLabel` turned `neoforge` into "Neoforge" |
| 37 | `backup_quota`/`used_backup_quota` constant `0` (Se) against real values (Si) | real values — otherwise the interface skips the quota check and the user sees the refusal only after the click |
| 38 | `status` without `deleting` (Se, E) against `status: "deleting"` (A) | four values; the vendor union has to be widened for it (17.1) |
| 39 | `flows.intro` constant `false` (Se) against writable (E `reset-to-setup`, A on `broken`) | a real field |

### 16.4 Error codes

| # | Contradiction | Decision |
|---|---|---|
| 40 | `unauthorized` (E, Si) against `unauthenticated` (all the others) | `unauthenticated` |
| 41 | `permission_denied` (D) against `forbidden` | `forbidden` — that holds for HTTP. As the `error.code` of an operation, `permission_denied` means something else and stays: not "you may not" but "the panel was not allowed to", an `EACCES` from the file system (5.11) |
| 42 | `not_found` as a catch-all code (Se, Si) against resource-specific codes | resource-specific; `not_found` stays only for paths in the file system |
| 43 | `operation_not_cancelable` (D) against `operation_not_cancellable` (A) — a pure typo conflict | double `l` |
| 44 | `disk_full` (Si, A) against `no_space` (D) | `no_space`, as the `error.code` of an operation too |
| 45 | `payload_too_large` (Se, A) against `file_too_large` (D, I) | `file_too_large` |
| 46 | `modrinth_unavailable`/`modrinth_rate_limited` (I) against `upstream_unavailable` (Se, E) | `upstream_unavailable`/`upstream_rate_limited`; which source it was is in the `message` |
| 47 | Six budget errors: `budget_exceeded` as 409 (Se) and 422 (A), `memory_budget_exceeded` 403 (E), `over_limit` 409 (Au), `user_over_limit` 422 (A), `user_over_budget` 409 (E) | two codes, both 409: `budget_exceeded` (this action would exceed the budget) and `over_limit` (the user is over already) |
| 48 | Port errors with overlapping meanings: `port_unavailable` means "taken or the pool is empty" in Se and "a foreign process" in E; on top of that `no_free_port` (A) against `port_pool_exhausted` (E) | four codes that do not overlap: `port_in_use`, `port_unavailable`, `port_pool_exhausted`, `port_out_of_pool` |
| 49 | `unknown_version` (Se) against `unsupported_version` (A) against `unsupported_game_version` (E) | `unsupported_game_version` |
| 50 | `validation_failed` (Au) against `invalid_request` (A, I) | `invalid_request`; the codes of `Au` that mean something of their own (`weak_password`, `invalid_role`, …) stay |

### 16.5 Console and WebSocket

These four stand outside the count, because `console.md` has already named and decided them as
contradictions itself; here only the confirmation:

* **The command goes over HTTP, not over the socket.** `Se` 3.3 claimed `command` was the only
  inbound message; `K` decides the opposite and gives its reasons. `K` holds.
* **That leaves the socket with no client→server direction**, and the WS message `error` from
  `Se` 3.1 loses its sender. Dropped.
* **The console messages are called `console*`, not `log`/`log4j`** (`Se` 3.5). A `log4j` stream
  does not exist here; our servers deliver raw text on stdout.
* **`world_id`** is the constant `"default"` in all eight documents — no contradiction, but what
  follows from it is one: `E` provides for `404 world_not_found`. The code goes, because no value
  ever reaches the backend.

---

## 17. Open decisions

Sixteen points this contract cannot decide on its own. Each with a recommendation.

### 17.1 Five changes to borrowed code

The plan says "borrow unchanged" (`docs/PLAN.md:71-76`). Five places cannot bear it:

| Place | what is missing or in the way | Recommendation |
|---|---|---|
| `server-settings/pages/general.vue:29-72,357,367,381` | the subdomain block. With `net.domain: ""`, `isValidSubdomain` is permanently `false` and `saveGeneral` bails out on its first line — **renaming is therefore impossible**, although the save bar does appear, and a red line "Subdomain must be at least 5 characters long." sits on the page permanently | Remove the block. A value five characters long would be no solution: then the hard-appended `.modrinth.gg` comes back (`ServerSubdomainLabel.vue:18`), and with it a write path to `changeSubdomain` that we do not have |
| `server-settings/pages/advanced.vue:5-89` | the SFTP block (`docs/PLAN.md:97`) | remove it; put a RAM slider between the startup command and the Java version there instead, otherwise `memory_mib` and `memory_max_mib` are dead in the contract |
| `ServerSetupModal.vue:65` | the seven loaders sit as a local constant in the vendor code, with no prop and no injection. Folia, Leaf and Velocity can therefore be picked when **creating** and not when **reinstalling** | patch one line — or live with the difference |
| `Archon.Servers.v0.Status` | knows no `deleting` (#38). Nothing happens at runtime, the branches only check for `'suspended'`; it is a pure type error on the first `tsc` | widen the union in the vendor copy |
| `BackupAdmonition.vue:24` | `operationId: number \| null`, ours is a ULID | leave it at the cast (10.1) — the component does not compute with the value |

Every one of these changes belongs in a change log, because every update of `packages/ui`
overwrites it.

### 17.2 Five routes we did not choose

`parseAuditEvent` and `AccessTable` produce `/user/<name>`, `/project/<slug>/version/<id>`,
`/hosting/manage/<id>/files?path=…` and `/hosting/manage/<id>/backups?backup=…` **hard-coded in
the source**, with no prop against it; `BackupItem.vue:78-82` links the author to `/user/<username>`
as well. `getUserProfileLink` cannot be switched off: if our function returns `undefined`, the `??`
branch takes over and exactly that dead link appears.

**Recommendation:** our server pages are called `/hosting/manage/:id/…` as well, or they get
redirects; `/user/:username` points at a plain user page of our own; `/project/…` goes to
modrinth.com. The frontend area decides, but somebody has to.

### 17.3 The Modrinth forwarding departs from the plan

`docs/PLAN.md:78-80` says `packages/api-client` is used "only with `labrinthBaseUrl` pointing at
the original". 8.15 sets the base URL to us instead. The client stays unchanged — only the target
changes. **Recommendation:** accept it and bring the plan along; the four reasons in 8.15 hold,
and the alternative costs one counter per browser window.

### 17.4 Three surfaces above feature parity

* **The log file picker** (6.4–6.6). Modrinth's own server interface does not have it; it comes
  from the desktop app. Without it the console stays fully usable (no `logSources` → no picker).
  **Recommendation: keep it**. Otherwise `onDelete` is a part of the contract nobody can reach,
  and everything before the last panel start would be out of reach in the console. Not needed for
  P1.
* **Automatic backups** (10.9/10.10). The plan is silent; the interface visibly distinguishes
  "Manual" from "Auto". **Recommendation: keep them**, default off. If they go, `automated` is
  constant `false` and there are two endpoints less.
* **A backup of the whole directory** instead of the world alone (`docs/PLAN.md:452`).
  **Recommendation: the whole directory** — the reasoning is in 10, and a world-only backup would
  protect against none of the operations `InlineBackupCreator` is offered in front of.

### 17.5 A fourth helper command `chown-tree`

The panel runs as `craftpanel`, the server as `craft-<id>`, and `craft-<id>` is **not** in the
group `craftpanel` (`docs/PLAN.md:160-161`). Everything the panel creates belongs to `craftpanel`
and is out of reach for the server process. On a restore that affects **every** file of the
server; but the helper's vocabulary knows only three commands, and `chown` is not one of them.

This is no small matter of the backup area but a gap in P0 — it hits every write the panel makes
into a server directory, and is only most visible there. **Recommendation:** a fourth command
`chown-tree <uid> <path>` that acts only below `/var/lib/<panel>/users/` and only on uids it
created itself. The alternatives fail: a second group per user on the same reasoning with which
the plan chose a single one (group membership is fixed when the process starts), and unpacking as
`craft-<id>` on the fact that a server user would then get read access to the backup files.

### 17.6 A byte budget per user

cgroups do not limit disk space, and the plan knows no disk budget. Today only `no_space` puts on
the brakes in front of a full disk. **Recommendation:** leave it for now; `size_bytes` is kept
from the start, so a later budget needs no data migration. If it comes, the check belongs in front
of 10.2 and 10.6, and the safety copy counts toward it.

### 17.7 The acceptance criterion of P6 is wrong

`docs/PLAN.md:483` demands "an editor can restart but cannot delete files". That is not to be had
with Modrinth's role sets: their `editorScopes` contain `FILES_WRITE`, and the role description
reads "Manage instance content, files, backups, and other settings". Rehanging the bits means
changing the description texts in Modrinth's components.

**Recommendation:** rewrite the criterion onto `viewer` ("a viewer can restart but cannot delete
files"); for `editor` you check "may delete files, but may not manage members and may not reset
the server". The same mechanics, a statement that has been tested.

### 17.8 Two disclosures somebody has to sign off

* **`GET /users/search` is open to everybody signed in** (3.5). Anybody can walk the list of names
  that way. Without the search there is no invitation. **Recommendation:** defensible on one
  machine among people who know each other; whoever wants it tighter needs a switch in 12.10.
* **"Share to mclo.gs" sends file contents straight from the browser** to a foreign service (6.7,
  and the same in the file editor). It cannot be rerouted, only switched off
  (`external_services_enabled`). **Recommendation:** default on, document the switch.

### 17.9 No panel-wide audit log

"limit changed", "user created", "user deleted" are recorded nowhere, because `events/` has no
renderer for them and a second log would be a second surface. Likewise missing is the **origin of
an admin action**: if an admin changes something on somebody else's server, they stand in the log
as an ordinary user — Modrinth's means for it (`actor.type = "support"`) comes with branding and
the label "Support (name)". **Recommendation for later:** the same table with `server_id = null`
and a plain list in the user administration.

### 17.10 "Resend" with no mailbox — **partly done**

Up to this round it held that there is no mail sending and no way to notify anybody outside the
panel; 11.5 only refreshes a timestamp. With section 19 there is mail sending, but **only for the
eight transactional mails from 19.12**. An invitation to a server (11.2, 11.5) still sends no
mail, and the label on "Resend" therefore stays as it is.

That is a decision and not an oversight: an invitation reaches its recipient in the panel, because
they have an account there — unlike a verification or a reset, which serve exactly the case that
somebody **cannot** get in. Whoever wants invitation mails hangs a ninth template on 19.12 and one
call on 11.2; no new scaffolding is needed for it.

### 17.11 The set of fields of `/admin/settings`

12.10 gathers four gaps that three area contracts point at without any of them defining the
endpoint. The individual fields have their reasons, the **cut** does not — somebody has to confirm
that the port pool, the operation width, the upload limit, `max_backups`, the switch for outgoing
services, `public_address`, `stop_grace_seconds` and `default_limits` belong together and do not
belong on two endpoints or in `config.toml`.

### 17.12 The eighth loader of the first wave

`docs/PLAN.md:379` counts eight loaders in the first wave, the table below it lists **seven**
(Vanilla, Paper, Folia, Purpur, Leaf, Fabric, Velocity), and the text speaks of "four sources"
while there are five. Our catalog has seven in wave 1. Which eighth was meant — Waterfall would be
the obvious candidate from the same PaperMC source — the plan has to say.

### 17.13 No canceling a running installation

There is no endpoint for it: `InstallingBanner` has no cancel button, and `DELETE` refuses in the
meantime with `409 server_busy`. Whoever wants to stop the download has to wait until the
operation runs into `broken`, or restart the panel (5.12). **Recommendation:** leave it; a cancel
that no foreign component can trigger would be dead code. For `server_create` the cancel exists,
because our own create page triggers it.

### 17.14 Panel-wide operations do not fit this model

Every operation has a `server_id`, and the delivery path is that server's socket. A self-update of
the panel or the creation of a system user without a server does not fit in. Today there is no
such operation. As soon as there is one, it needs either `server_id: null` along with a delivery
path of its own — and with it the second socket that 13 rules out — or the thing is not kept as an
operation. **Deliberately left open.**

### 17.15 The hourly half of the cool-down from 21.5 cannot fire

21.5 promises "one mail per 60 s, **five per hour** per account", counted over the `created_at` of
the rows in `password_resets`. The minute half holds. The hourly half cannot fire: `mint`
(`auth/reset.rs`) asks `too_soon` and **afterwards** deletes every row of the account with
`forget_all` before it writes the new one (that is the rule "one open token per account"). The
number `too_soon` counts is therefore at most 1 and never 5.

**Measured** (probe over `service.begin` with timestamps 61 s apart, all within one hour,
`mail_outbox` and `password_resets` counted after every step):

```
Step 0: rows 1, mails 1     Step 4: rows 1, mails 5
Step 1: rows 1, mails 2     Step 5: rows 0, mails 5
Step 2: rows 1, mails 3     Step 6: rows 0, mails 5
Step 3: rows 1, mails 4     Step 7: rows 0, mails 5
```

The mails stop at five, but not because of 21.5, because of the mail brake from 19.10 (five per
address and kind per hour). And because the brake sits **behind** the minting, step 5 deleted the
living token of the account and the replacement mail was refused: from there on the account holds
**no** valid token, and a stranger who knows the address keeps it in that state with one request
per 60 s (the brake from 21.6.4 lets ten attempts per 15 min through). Whoever has forgotten their
password then never gets a usable link.

**Two ways out, and the choice belongs in a round of its own**, because both touch the retention
of the tokens — the property "works exactly once" has hung by a thread here once before
(`docs/PASSWORD-RESET.md` 3.1):

1. **Let old tokens expire instead of deleting them** (`expires_at = now`, `used_at` untouched).
   `living` refuses them, "one open one per account" still holds, the sweeper takes them after
   24 h, and `too_soon` has something to count again. Costs an adjustment to
   `a_fresh_request_makes_the_older_link_worthless`, which today counts rows instead of living
   tokens.
2. **Count the hour where it cannot be deleted**: `mail_outbox`, the same row 19.10 already counts
   (address + kind + `created_at`). Then the two numbers agree by construction, and the check sits
   before the deletion. Costs a fourth call at the seam of 19.14.

**21.4 is affected too**, and that was written down wrongly here at first. Its own bucket
(`rates::ADMIN_RESET_PER_ACCOUNT`) does sit before the deletion, but it counts only the admin's
presses, and the brake from 19.10 counts `mail_outbox` per address and kind. The **public** form
fills that one as well. Once it is exhausted, `on_behalf_of` walks past its own bucket (counter 0),
deletes the living token, mints a new one, and only then does the outbox refuse; the replacement
token falls with the refusal, the old one is gone already.

**Measured** (probe in `api/recovery.rs`: five `reset_password` mails to the same address placed
into the hour, then a living token minted, then the button of 21.4 pressed **once**):

```
before:   rows 1, mails 5
answer:   429 mail_rate_limited — "5 mails of this kind went to this address in the last hour."
after:    rows 0, mails 5
the token the user had in the mailbox: 400 invalid_reset_token
```

So a stranger sets the trap at the public form and the admin springs it — with a button that is
not gray, because `configured()` and `can_link()` both answer green. Way 2 above closes this along
with it; way 1 does not, there `on_behalf_of` has to ask the brake **before** the deletion as
well. The choice belongs in the same round.

### 17.16 `cpu_mode: "share"` only half arrives in the kernel

The table in 12.7 promises two things for the share mode: `cpu.max` goes away **and** `cpu.weight`
gets the share of this account. Today only the first happens.
`craftpanel_proto::ResourceLimits` has no field for `cpu.weight` (`auth/limits.rs:137-143`), so in
share mode every account keeps the kernel's default weight and, when it gets crowded, shares **in
equal parts** instead of by its cores. Two accounts with 1 and with 8 cores then get the same.

That only becomes visible on a machine where things get tight, and only for accounts whose
`cpu_mode` is `share` — the default from 0002 is `cap`. It is no gap in the isolation: a missing
ceiling is still not a ceiling, and `cpu.max` is deliberately gone there anyway. It is a broken
promise to the admin who moved the slider.

**Recommendation:** a fifth field `cpu_weight: Option<u32>` in `ResourceLimits`, written by the
helper into `cpu.weight`, with the same rule "not given means `max`/default". That touches the
protocol version between panel and helper (`HELPER_PROTOCOL_VERSION`), so it belongs in a round in
which both binaries ship together anyway. Until then 12.7 says what really happens.
`docs/PLAN.md:345-349` also says today "both are the same line of code" — the draft was meant, not
what exists.

---

## 18. playit.gg — public addresses

The design, the measurements and playit's own answers are in `docs/PLAYIT.md`; here there are only
the endpoints, their permissions and their error codes. `18.n` is the same as `PLAYIT.md` `8.n`.

**One account per panel user.** The panel provides none: whoever wants an address for their
servers that carries from outside connects their own playit.gg account, and their four ports
(sixteen with premium) are theirs. A panel admin can connect an account for nobody — the
confirmation happens in the browser of the person the account belongs to. Two ways are left to
them: read the overview (18.10) and disconnect somebody else's account (18.11), because otherwise
a suspended user holds four ports and a running tunnel service forever.

All eleven endpoints answer `409 external_services_disabled` as long as the panel-wide switch from
12.10 is off — with one exception: `?tunnels=keep` in 18.5 and 18.11 touches nobody and is
therefore the way out of the corner when playit.gg cannot be reached.

### 18.1 `GET /api/v1/playit`

Permission: signed in, and it is always your own state. Response `200` `PlayitStatus`.

Reads one row and calls nobody. Never carries the key and never a field the key could stand in;
`claim` is your own sign-in process, if one is running.

### 18.2 `POST /api/v1/playit/claim`

Permission: signed in. No body. Response `201` `PlayitClaim`.

`201` and not `202`: the URL **is** the answer and is complete at the moment it is handed over.
What keeps running behind it only fills in the state. Errors: `409 playit_already_claimed` (this
user has a key already), `429 upstream_rate_limited`, `502 upstream_unavailable`.

The panel holds at most four sign-in loops at a time; whoever gets none keeps their row and their
URL and is polled as soon as one comes free. The deadline of fifteen minutes runs from
`started_at`, so it runs during that too.

### 18.3 `GET /api/v1/playit/claim`

Permission: signed in. Response `200` `PlayitClaim`, otherwise `404 playit_claim_not_found`.

The `404` means "no process open" — including after the one that worked. Whether it worked is said
by `configured` in 18.1 alone.

### 18.4 `DELETE /api/v1/playit/claim`

Permission: signed in. Response `204`, otherwise `404 playit_claim_not_found`.

### 18.5 `DELETE /api/v1/playit?tunnels=delete|keep`

Permission: signed in. Response `204`.

The decision rides in the query so that no body hangs off a `DELETE` — the same shape as 12.6.
Without it, a tunnel that is still issued is a refusal (`409 playit_has_tunnels`) and not a choice
made silently. `delete` gives the addresses back at playit, `keep` leaves them standing there and
the ports taken. Errors besides: `400 invalid_request` for any other word,
`409 playit_not_configured`, `502 upstream_unavailable`.

### 18.6 `POST /api/v1/playit/agent/restart`

Permission: signed in. No body. Response `202` `PlayitStatus`. Errors:
`409 playit_not_configured`.

### 18.7 `GET /api/v1/servers/:id/playit`

Permission: `BASE_READ`. Response `200` `ServerTunnel`; `state: "none"` is a server without an
address and not an error. Carries **no** port counts: what is left of the owner's budget is none
of a viewer's business.

### 18.8 `POST /api/v1/servers/:id/playit`

Permission: owner or panel admin. No body, and in particular **no port** — the target of the
tunnel comes from `allocations.is_primary` and from nowhere else. Response `202` `ServerTunnel`
with `state: "pending"`; the address is there seconds later and comes over 18.7.

The tunnel is created on the owner's account. Errors: `409 playit_not_configured` (the owner has
connected no account), `409 playit_tunnel_exists`,
`409 playit_port_limit` (their account has no free port left, with the numbers in the sentence),
`409 playit_no_primary_port`, `502 upstream_unavailable`.

### 18.9 `DELETE /api/v1/servers/:id/playit`

Permission: owner or panel admin. Response `204`. Errors: `404 playit_tunnel_not_found`,
`502 upstream_unavailable`.

It is given back over the account that carries the tunnel. That is the owner — except for an
address from the old panel-wide account, which the admin who took it over carries (`PLAYIT.md` 7).

### 18.10 `GET /api/v1/admin/playit`

Permission: panel admin. Response `200` `PlayitOverview[]`, one row per user with a connected
account, sorted by username.

No `claim`: a sign-in code is a way into somebody else's account. No field that could carry the
key. It reads from the database; `agent` and `configured` are what this panel process knows.

### 18.11 `DELETE /api/v1/admin/playit/:user_id?tunnels=delete|keep`

Permission: panel admin. Response `204`. Errors as in 18.5, plus `404 user_not_found`.

Writes a `warn!` line with the actor and the target. An audit entry in the sense of 11.9 is **not
possible**: `audit::record` demands a `server_id`, and there is no panel-wide audit log (17.14 is
the same gap). That is a gap and not an oversight; covering it with an entry on some arbitrary
server would be an invention.

---

## 19. Sending mail through Resend

The design with the measurements against Resend is in `docs/MAIL.md`; here are the endpoints,
their permissions and their error codes. All eight are **panel admin**, and not one of them ever
carries a single character of the key.

**The key is not in the database.** It sits as a file with `0600` in a `0700` directory under
`<data_dir>/mail/`, exactly like playit's key
(`migrations/0008_playit_per_user.sql`), so that a copy of `panel.db` — for a bug report, for a
backup — contains no way into Resend. Not even a short form like `re_…AB12`: with exactly one key
in the whole panel, a leftover hint gains nothing and would be part of the secret in every copy.

**Not set up is a normal state.** Without a key nothing starts, nothing calls outside and nothing
crashes; there is no banner and no red tile. Only whoever wants to trigger a mail gets a clear
refusal (`409 mail_not_configured`). The same stance as playit
(`crates/craftpanel/src/playit/mod.rs:11-13`).

**Sending mail does not hang on `external_services_enabled`** (reasoning in 12.10).

### 19.1 What works without a verified domain, and what does not

Evidence retrieved 2026-08-13. Resend demands "at least one domain" for real sending
(resend.com/docs/dashboard/domains/introduction). As long as none is verified:

* The sender may only be `onboarding@resend.dev`.
* The recipient may only be **the address the Resend account was opened with**; anything else
  Resend answers with `403` and its own name `validation_error` ("You can only send testing emails
  to your own email address").
* A sender of your own without verification gives the same `403 validation_error` ("The
  `domain.com` domain is not verified").

From this follows the default `from_address = onboarding@resend.dev`: on the first day, with no
domain, the operator can press the button from 19.5 and see that it works. Sign-up mails to other
people's addresses only go out after the domain verification (MX and TXT records, SPF and DKIM).

**The panel does not ask for this state, it learns it from the answer to a send.** A surface over
Resend's `GET /domains` would need a key with `full_access`; we want `sending_access` ("Can only
send emails",
resend.com/docs/api-reference/api-keys/create-api-key), and with that every domain query would be
Resend's `401 restricted_api_key` and the surface permanently red. There is therefore no domain
administration in the panel (19.13).

Numbers that belong in the surface because they explain the behavior: free tier **100 mails a day,
3,000 a month, one domain** (resend.com/pricing); rate limit **10 requests per second per team**
(resend.com/docs/api-reference/introduction).

### 19.2 `GET /api/v1/admin/mail`

Permission: panel admin. Response `200` `MailSettings`.

`state` is the one field the interface hangs on:

| `state` | means |
|---|---|
| `not_configured` | no key file — the normal case on the first day. The **file** decides and not `key_set_at`: a row that claims a key which does not exist would send the panel outside empty-handed. The sender address does not belong in this question, it cannot be missing (default `onboarding@resend.dev`, and 19.3 takes no empty one) |
| `configured` | a key is here. Does **not** say that it still works (the same honesty as `PlayitStatus.configured`, `playit/mod.rs:103-104`) |
| `file_sink` | `CRAFTPANEL_MAIL_SINK` is set: every mail lands as a file and **no** request goes onto the network |

`file_sink` is deliberately a state of its own and not a silent redirect. With it the whole sign-up
runs through without a Resend account, clickable verification link and all — the way the operator
can already go today. At startup the panel writes a clear log line about it.

`link_base` is the basis of every link in a mail and **not** `panel_settings.public_address`. That
is the most important decision of this section: `public_address` becomes `Server.net.ip` (12.10),
that is the address players type in — usually a name or an IP without a scheme, often a LAN
address. Guessing a URL out of it would hit `http` instead of `https` or would go past a reverse
proxy. If `link_base` is empty, **no mail with a link** goes out (`409 mail_no_link_base`); the
three mails without a link (`account_rejected`, `password_changed`, `test`) keep going.
`example_link` shows what comes out of it, so that the operator sees the mistake before a user
sees it. There is no second, derived address: `link_base` is the only source.

**A missing `link_base` therefore switches off two whole areas, and it does so at the front rather
than at the back.** The two questions are separate (`mail::Mail`):

| Question | Function | true when |
|---|---|---|
| May a mail go out at all? | `configured()` | a key file is here — or `CRAFTPANEL_MAIL_SINK` turns every mail into a file |
| May an area be offered whose mail carries a **link**? | `can_link()` | `configured()` **and** `link_base` is set |

`can_link()` is the readiness that `registration_enabled` and `password_reset_enabled` in 20.1 hang
on, not `configured()`. The reason is that the alternative sends mails into nothing: an applicant
who fills in a form that cannot build their verification gets `409 mail_no_link_base` on sending,
where somebody with an existing account gets `202` — an oracle about other people's addresses
(20.11), and even without that they would never get to an account. The same for the reset: a
token nobody can learn about is a row waiting to be stolen, so without `can_link()` **none** comes
into being in 21.1. Affected are the five mails with a link — `verify_email`,
`address_already_registered`, `account_awaiting_review`, `account_approved`, `reset_password`; the
three without (`account_rejected`, `password_changed`, `test`) go out without `link_base` and then
carry a footer without an address.

An admin may see the difference, a stranger may not: 21.4 asks only `configured()` and answers the
admin honestly with `409 mail_no_link_base` when the address is missing.

`sent_today`, `queued` and `failed` are counts over `mail_outbox` (19.10). There is no field for
the key; `key_set_at` only says when the file was written.

### 19.3 `PUT /api/v1/admin/mail`

Permission: panel admin. Request `UpdateMailSettingsRequest`, response `200` `MailSettings`.

`api_key` has three meanings, and all three have to be there: `null` (field left out) means
**unchanged**. Otherwise every save of the sender address would delete the key; `""` means
**delete**; a text means **replace**. Writing goes as in `playit/agent.rs:345-359`: create the
directory, `0700`, write into `api_key.part`, `0600`, `sync_all`, then `rename`, so that half a
file is never read.

Cleaning on save, because foreign text ends up in a mail header: `from_name`, `from_address` and
`reply_to` lose CR, LF and ASCII control characters; `from_name` also `< > " ,`. The address check
stays thin (exactly one `@`, no whitespace, neither side empty, a dot in the domain, at most 254
characters) — the real check is Resend's answer, and that is wired up in 19.11.

Errors: `400 invalid_request` — a broken `from_address`, a broken `reply_to` (empty means "none"),
a negative `daily_limit`, a `link_base` without `http://` or `https://`, or an `api_key` that
cannot be one (empty after trimming counts as **delete**; whitespace and control characters in the
middle of the text are the error). All refusals fall **before** anything is written: half a save
is a save nobody wanted.

An `http://` `link_base` is **accepted and not refused**: on a home network it is the only
possible value. The surface says one sentence about it, because over `http` the token travels in
the clear.

### 19.4 `DELETE /api/v1/admin/mail/key`

Permission: panel admin. Response `204`, even when there was no file — deleting is idempotent
(`playit/agent.rs:366-371` does it the same way). Afterwards `key_set_at` is `null` and `state` is
`not_configured`, and sign-up is closed with it (20.1). The confirmation dialog says so.

### 19.5 `POST /api/v1/admin/mail/test`

Permission: panel admin. Request `SendTestMailRequest` (`to` may be left out → your own address
from 3.3). Response `200 {"id": "<Resend-id>", "to": "…"}`.

**This one mail is sent directly, not queued.** Reason: here the foreign answer *is* the result.
Queued, the button would say "saved" and a wrong key would go unnoticed — exactly the mistake the
button is meant to prevent. It is a real mail in our shell (template `test`), counts toward the
daily amount and stands as a row in the outbox.

Errors: `409 mail_not_configured`, `400 invalid_request` (neither `to` nor an address of your
own), `429 mail_rate_limited` (more than ten test mails in 60 minutes — otherwise the button is a
small mail cannon on Resend's bill), `429 mail_quota_reached`, and the five translations of
Resend's refusals from 19.11: `502 mail_key_rejected`, `502 mail_sender_rejected`,
`502 mail_refused`, `502 mail_upstream`, `502 mail_unreadable`. `message` is in every case the
sentence from 19.11.

### 19.6 `GET /api/v1/admin/mail/outbox?limit=&state=`

Permission: panel admin. Response `200` `MailOutboxList`, newest first, `limit ≤ 200` (default 50),
`state` filters on one of the four states. Form 1 from 1.8. A `limit` that is too large is trimmed
and not refused (as in 12.2); a `state` that does not exist is `400 invalid_request` — trimming
would mean guessing here, and an empty list would be a wrong answer.

The list carries recipient, kind, state, attempts and the error sentence, not the body. It is the
log of this area; an entry in the audit log after 11.9 is impossible, because `audit_log` demands
a `server_id` (17.9).

### 19.7 `GET /api/v1/admin/mail/outbox/:id/content`

Permission: panel admin. Response `200 text/html` — the HTML that was really sent or that stayed
behind. The way for "I did not get anything". Errors: `404 mail_not_found`,
`404 mail_content_gone`.

`mail_content_gone` is the normal case for a delivered mail and not a defect: after success `html`
and `text` are set to `NULL`, because the body carries the link in the clear while the token
itself sits only as a hash in its table (`auth/session.rs:3-6`).

That names a disclosure you have to know about: **between queuing and delivery a panel admin can
read a valid reset link over this endpoint.** It gives them no new power — over 12.5 they may set
a password on any account. The window is kept small all the same: success empties the body, the
retry window ends after at most ~2.5 hours (19.10), and whoever copies the database while a mail
is in flight copies this link along with it.

### 19.8 `POST /api/v1/admin/mail/outbox/:id/retry`

Permission: panel admin. No body. Response `202`, the row back to `queued`, the counter to `0`.

Only for `state = 'failed'`; everything else is `409 invalid_state`, and a row whose body has been
emptied already is `404 mail_content_gone` — without a body there is nothing to send. Errors
besides: `404 mail_not_found`.

### 19.9 `GET /api/v1/admin/mail/preview/:kind`

Permission: panel admin. Response `200 text/html` with **example values**, no key, no network, no
database. `:kind` is one of the eight values from 19.12, otherwise `404 not_found`.

That is the second of three ways to see the design without a Resend account. The other two are
`craftpanel mail preview [--out DIR]` (writes all sixteen files into `/tmp/craftpanel-mail`) and
`CRAFTPANEL_MAIL_SINK` (19.2).

### 19.10 The queue, the brakes and the retention

**Everything is queued except the test mail.** A sign-up must not hang on Resend's runtime:
queuing is an `INSERT`, a call to the outside is 5 s of connect and 20 s of response deadline. And
a lost reset mail locks somebody out — a queue in memory loses it on a restart, the row in the
database survives one.

One worker in the panel process, woken on queuing and on top of that every 30 s for due retries;
at most **two sends per second** (a fifth of Resend's limit) and never more than one request at a
time, in `created_at` order.

State path per row: `queued → sending → sent | queued (+retry) | failed`.

* **The `Idempotency-Key` is the row's ULID.** That covers the case our own restart creates: at
  startup, rows left hanging in `sending` are set back to `queued`, because nobody knows whether
  Resend has the mail already. Within 24 hours Resend answers with the same id and sends **no**
  second mail (resend.com/docs/dashboard/emails/idempotency-keys).
* **Only what is temporary is retried**: 30 s, 2 min, 8 min, 30 min, 2 h — five attempts, then
  `failed`. Everything permanent (wrong key, unverified domain, refused content) becomes `failed`
  **at once**: twenty attempts with the same wrong key help nobody, and the admin has to do
  something.
* `daily_quota_exceeded` is the special case: the counter does not rise, instead `next_attempt_at`
  moves to the start of the next UTC day.
* **On success `html` and `text` are set to `NULL`** (19.7).
* Rows older than **30 days** are deleted daily, in the pattern of `audit::spawn_purge`
  (`audit/mod.rs:42`). Shorter than the 180 days of the audit log, because addresses sit here.

Three brakes, all counted with `SELECT count(*) FROM mail_outbox WHERE …` — durable across
restarts and without a second data structure, which the sign-in brake cannot say of itself
(`auth/brake.rs:5-7`):

| Brake | Limit | Response |
|---|---|---|
| per address and kind | 5 in 60 minutes | `429 mail_rate_limited` |
| test mails | 10 in 60 minutes | `429 mail_rate_limited` |
| panel-wide | `daily_limit` (default 100) in 24 hours | `429 mail_quota_reached` |

These three refusals come back **at the queuing**, that is where the caller can still decide
whether to create the account at all (19.14).

### 19.11 Resend's answers and the sentence the admin reads

This table is the contract of the mail module; every row is a test with a file in
`mail/testdata/`. Foreign texts are cut to 200 characters (`playit/http.rs:158`) so that no
foreign text bursts a column.

The column `name` is **Resend's** own error name, none from 1.7; our codes are in the third
column.

| HTTP | Resend's `name` | our code | Sentence to the admin (`message`, and `last_error` in 19.2) |
|---|---|---|---|
| 401 | `missing_api_key` | `mail_key_rejected` | Resend saw no key. Enter one under Administration → Mail. |
| 401 | `restricted_api_key` | `mail_key_rejected` | This key may not send. Create one with "Sending access" at resend.com/api-keys. |
| 403 | `invalid_api_key` | `mail_key_rejected` | Resend does not know this key (any more). Create a new one and enter it here. |
| 403 | `validation_error`, text contains `not verified` | `mail_sender_rejected` | The domain of ⟨from⟩ is not verified at Resend. Verify it at resend.com/domains — or take `onboarding@resend.dev` for the first attempt and send only to the address of your Resend account. |
| 403 | `validation_error`, text contains `own email address` | `mail_sender_rejected` | Without a verified domain, Resend only accepts the address you opened your Resend account with. |
| 422 | `invalid_from_address` | `mail_sender_rejected` | Resend does not take ⟨from⟩ for an address. Form: `name@domain.tld` or `Name <name@domain.tld>`. |
| 429 | `rate_limit_exceeded` | `mail_upstream` | Resend is throttling us (10 requests per second). The mail waits and will be tried again. |
| 429 | `daily_quota_exceeded` | `mail_quota_reached` | Resend's daily amount is used up (free tier: 100 a day). The mail waits until tomorrow. |
| 429 | `monthly_quota_exceeded` | `mail_quota_reached` | Resend's monthly amount is used up (free tier: 3,000). No mail goes out until the month turns. |
| 451 | `security_error` | `mail_refused` | Resend refused the mail as a security problem. The text has to be changed. |
| 400/422 | `validation_error`, `missing_required_field`, `invalid_parameter` | `mail_refused` | Resend refused the request: ⟨text⟩. That is a fault of the panel, not a setting. |
| 409 | `concurrent_idempotent_requests` | `mail_upstream` | The same mail is already running. It will be looked at again shortly. |
| 409 | `invalid_idempotent_request` | `mail_refused` | Duplicate key with different content — the mail is queued again. |
| 5xx | anything | `mail_upstream` | Resend had an error (⟨status⟩). The mail waits and will be tried again. |
| — | network, timeout | `mail_upstream` | api.resend.com could not be reached: ⟨reason⟩. The mail waits. |
| 2xx | no `id` in the body | `mail_unreadable` | Resend answered in a form we do not understand. |

`mail_upstream` and `mail_quota_reached` are **temporary** (retry per 19.10), the other three are
**permanent** (`failed` at once).

Two distinctions hang on pieces of text (`not verified`, `own email address`). If Resend changes
the wording, the message falls back to the general sentence — nothing crashes, the advice goes
blurry. The `#[ignore]` test against the real service is the place that notices it (pattern
`playit/http.rs:267-311`).

### 19.12 The eight mails

Language **English**, like the whole interface (`web/src/i18n.ts:13`). A second language doubles
sixteen files and needs a field "language" per user; that is a decision of its own and a round of
its own.

| `kind` | Subject | who gets it | Link |
|---|---|---|---|
| `verify_email` | Confirm your email address | the applicant (20.2) | `/verify-email` |
| `address_already_registered` | Someone tried to sign up with your address | the owner of an existing account (20.2) | `/login`, `/forgot-password` |
| `account_awaiting_review` | A new account is waiting for you | every panel admin with an address (20.3) | `/admin/registrations` |
| `account_approved` | Your account is ready | the applicant (20.6) | `/login` |
| `account_rejected` | About your sign-up | the applicant (20.7), neutral and **with no reason** | none |
| `reset_password` | Reset your password | the account holder (21.1) | `/reset-password` |
| `password_changed` | Your password was changed | the account holder (3.4, 12.5, 21.3) | none, only `link_base` |
| `test` | Test mail from your panel | the admin (19.5) | none |

Subject and preview text sit in the Rust part next to the values, not in the template. Otherwise
changing a subject would mean reading HTML. The footer of every mail: "This mail comes from the
panel at ⟨link_base⟩, because an account with this address exists there." No unsubscribe link: all
eight are transactional mails, there is nothing to subscribe to.

**Design.** The shell and the color table come verbatim from Modrinth's own mail templates
(`/root/ref-modrinth/apps/frontend/src/templates/emails/**`, GPL-3.0-only like the rest, noted in
`COPYING.md`), but as pieces of HTML with `include_str!` instead of as Vue components: Modrinth's
shell carries its colors as hand-entered hex values anyway, the Vue way would bring Node and two
0.0.x packages into the Rust build, and mails can use nothing from our interface — no flexbox, no
CSS variables, no web font. That it is the same design is secured by a test that checks the color
table against `vendor/modrinth/assets/styles/variables.scss`; the yardstick therefore lies outside
what is being measured. **No images, none at all** — `link_base` can be a LAN address, and blocked
images are the default in many places. The wordmark header is text.

### 19.13 What is explicitly not built

* **No bounce handling.** What is visible is "accepted by Resend", not "arrived in the mailbox".
  Webhooks would need a public, unauthenticated endpoint with signature checking,
  `GET /emails/{id}` would need `full_access`. The surface labels it exactly that way.
* **No domain administration** (19.1).
* **No second provider, no SMTP.** The client sits behind a struct `Outgoing`; a second provider
  later is a module and not a rebuild.
* **No attachment, no scheduled sending, no batch sending.**
* **No invitation mails** (17.10).

### 19.14 The seam: how the other areas trigger a mail

Exactly four calls, and no area but this one talks to Resend:

```rust
mail.configured().await -> bool   // does a mail go out at all? (21.4)
mail.can_link().await -> bool     // may an area with a link be offered? (20.1)
mail.send(Message::…).await -> Result<Id, Failure>
mail.notify(Message::…).await    // errors only into the log, never upward
```

There are two questions because there are two answers (19.2): 20.1 asks `can_link`, because both
areas live off a link; 21.4 asks `configured`, because an admin is standing at that button, and
you may tell them that the panel address is missing.

`send` for everything a flow depends on (verification, reset, approval): the refusal is one of the
codes from 1.7, and the caller decides whether to create the account at all. `notify` for the
trimmings (`password_changed`, `account_awaiting_review`): a mail that does not go must not turn a
`204` into a `500` — the same rule as with the audit log (`audit/mod.rs:17-19`).

**This area builds the link**, not the caller: the caller gives `("/verify-email", token)`, the
mail module puts `link_base` in front of it and refuses when there is none. That puts the rule
"no panel address, no mail with a link" in one place and not in three handlers.

---

## 20. Accounts: sign-up, verification, approval

The design is in `docs/SIGN-UP.md`. Seven endpoints, four of them without a session.

**The load-bearing decision: an open application is not a row in `users`.** It is a row in
`registrations`, and the `users` row only comes into being once the account becomes usable. The
reasoning is in what exists, not in taste:

* `users::reconcile` (`auth/users.rs:463-484`) looks on **every** panel start for rows with
  `system_state = 'provisioning'` and creates a system user for each of them. Half an application
  in `users` would get exactly the system account it must not get on the next restart.
* `users::search` (`auth/users.rs:134`) serves 3.5, the invitation path onto other people's
  servers. An account without a system user would be invitable.
* `page()`, `promised()` and the disk sum (`auth/users.rs:104`, `api/admin.rs:210-232`,
  `auth/disk.rs`) would count half accounts as well; `HostCapacity.allocated` would claim disk
  given away, and 12.1 says literally "what the admin has given away".

With a table of its own **none** of these queries changes. The price is one namespace across two
tables (20.10).

**Never an admin, secured three times over:** `registrations` has no role column — there is
nothing there that could carry a role; the admission sets `PanelRole::User` as a literal; and a
test sends `panel_role: "admin"` in the body of 20.2 and checks that the admitted account is
`user`.

### 20.1 `GET /api/v1/auth/options`

Without a session. Response `200` `AuthOptions`.

**One** endpoint for everything the sign-in page has to know before anybody is signed in, not
two. `registration_enabled` is already the **conjunction**, not the switch from 12.10:

```
registration_enabled = panel_settings.registration_enabled AND mail.can_link()
password_reset_enabled = mail.can_link()

mail.can_link() = mail.configured() AND the panel_settings row has a link_base   (19.2)
```

The panel offers nothing it cannot carry through; a verification mail that never goes out would be
an account nobody gets into. `password_reset_enabled` comes from the same readiness (21.1),
`registration_requires_approval` tells the applicant before sending that an approval follows.

**The third condition is a `link_base` that is set**, and not merely `mail.configured()`: both
areas live off a mail with a **link** in it, and a panel with a key but without an address of its
own cannot build exactly those four mails (19.2, `mail::Mail::can_link`). If the form were open
anyway, it would cost twice. The applicant would get `409 mail_no_link_base` on sending, where
somebody with an existing account gets `202` — that is the oracle 20.11 forbids. And they would
never arrive: without a link there is no verification and therefore no account. That is why the
answer is a closed form with one sentence instead of a mail into nothing.

The endpoint carries **no** number and no name: no `user_count`, no panel name, no address. It is
reachable without a session, so everything in it is public.

### 20.2 `POST /api/v1/auth/register`

Without a session. Request `RegisterRequest`, response **`202`** `{"status": "check_your_email"}`.

This answer is **the same byte for byte** for a new, a known and a blocked address (20.11). If the
address already carries an **account**, no new row comes into being; instead
`address_already_registered` goes to the existing address — the owner learns that somebody has
used their address, and gets the way to the sign-in and to the reset. If it carries an open, still
**unverified application**, the answer is the same, but the row is replaced; see below.

`username`, by contrast, is refused **honestly**: `409 username_taken`. Names are visible to
everybody signed in over 3.5 anyway, and a form that may not check the name is unusable. The name
is checked against open applications too (`users::claim_name` asks both tables), with a sentence
of its own, "reserved for an open sign-up", and the same code.

Errors: `409 registration_disabled` (switch off **or** no mail sending — one code, because the
difference is none of the applicant's business), `400 invalid_request` (name 3–39 characters
`[a-z0-9_-]`), `400 invalid_email`, `400 weak_password` (minimum length 10, as in 1.7),
`409 username_taken`, `429 rate_limited` with `Retry-After`, `415 unsupported_media_type`.

**argon2 runs in every case** — for a known address and for a block as well. Otherwise the
response time is the directory. This is checked with the existing counter
`password::verifications()` (`auth/password.rs:96`), exactly as `api/session.rs:290-313` does it
for the sign-in.

**An unverified application holds nothing for anybody.** If this address already carries one, this
form replaces it **entirely** — name, password and token together
(`registration/store.rs::take_over`). That was the hole that stood here before: a verification
nobody has answered proves nothing about who reads that mailbox, and must therefore not decide
whose account the address becomes. Leaving the old row standing and merely sending it a fresh link
would turn a stranger's application into somebody else's account: the stranger applies with an
address that is not theirs, the owner of the address applies for themselves, and the mail that
then arrives confirms the stranger's name and the stranger's password.

The trade this costs is the smaller one: a stranger can overwrite an application that is still
unverified and make the link in the applicant's mailbox worthless. That costs the applicant
another trip to the form, goes no faster than 20.11 allows, and ends the moment the account
exists — from then on the address belongs to `users`, and every further form is answered with a
mail to its owner and with nothing else.

Three conditions carry this, and every single one is needed:

* **`state = 'email_unverified'`.** An address that is *verified* was verified by the person who
  reads the mailbox; nothing a stranger types may take that away from them — neither their place
  in the queue nor the verification itself (counter-check `registration/tests.rs:578-581`).
* **`email = ?`.** The row is the row of *this* address; a caller cannot hand in an ID and an
  address that do not belong together. And because the comparison runs on the normal form from
  20.10, `MAX@…` finds the same row as well. Otherwise the stranger's application would stay
  standing next to it (`registration/tests.rs:509-511`).
* **`tokens_sent` starts from the beginning.** Five links that the previous applicant used up
  would leave the new one with a row that can never send them one (20.9) — a stranger would make
  an address unusable for everybody by requesting their own link five times
  (`registration/tests.rs:728-731`). `created_at` is likewise that of the new application: the
  seven days from 20.12 belong to the open application, not to the one it replaced.

Because `take_over` swaps name, hash and token **in one statement**, everything that happens
*after* a token is looked up has to hang on the token and not on the ID: the hash the admission
writes into the account, the verification that puts a row into the queue, and the name with which
the resent link greets in its mail. Whoever asks for one of those by the ID gets, for a row that
has changed owner in between, the answer for the wrong person — an account whose name comes from
one application and whose password from the other (`registration/store.rs:175-200,229-234`,
counter-check `registration/tests.rs:660-666`).

And because an applicant who has lost their mail fills in the same form once more, their own open
application must not answer them with `username_taken`. Without this exception in
`claim_name_for_sign_up` only the resend from 20.4 would be left to them, and that is exactly the
door the take-over closes. The price is named: `202` instead of `409` tells somebody who already
knows a name that has not been given out that this name sits on the address they have just
guessed. Asking destroys the application that was asked about, so the question is anything but
quiet, and what 20.11 is really about stays untouched — a guess at an **address** is answered with
`202` either way (`registration/mod.rs:177-193`, counter-check `registration/tests.rs:622-625`).

If the mail sending fails, the row just written **goes back with the mail**, and the refusal stays
inside. Two reasons, each enough on its own: to the outside a `409` or `429` here would be a
directory of the people who have an account here, at the price of one request per address (20.11)
— and leaving the row standing would hold name and address for seven days, so that the applicant's
second attempt would be turned away with `username_taken` by their own dead application
(`registration/mod.rs:274-284`, counter-check `registration/tests.rs:281-286`).

### 20.3 `POST /api/v1/auth/verify-email`

Without a session. Request `{"token": "…"}` — the token travels in the **body**, never in a server
URL (1.2). Response `200` `{"state": "active" | "awaiting_approval"}`.

Two ways, depending on `registration_requires_approval`:

* **off** → the `users` row and the system user come into being now (20.13), the application row
  is deleted, response `state: "active"`. No mail goes out: the person is standing in front of the
  screen.
* **on** → the row goes to `awaiting_approval`, response `state: "awaiting_approval"`, and
  `account_awaiting_review` goes to **every panel admin with an address** (`notify`, so that a mail
  error cannot spoil this answer).

**No session, no cookie**, not even in the `active` case. The link comes out of a mailbox an
attacker can have, and there should be exactly one way that hands out sessions
(`auth/session.rs:29`). The page shows "your account is ready" and a button to `/login`.

Errors: `404 invalid_token`, `410 token_expired`, `409 registration_disabled`,
`409 username_taken` (the name was given out by hand in the meantime — the application row stays,
the page says "the name has been taken since, please sign up again"), `429 rate_limited`.

The `404` is also the answer to a **second** click after an admission without approval: the row is
gone then. `message` therefore says literally what to do ("If you have already confirmed, sign
in."). If the application is waiting for approval, a second click is `200` with the same body
instead — the row is alive and answers it (20.9).

### 20.4 `POST /api/v1/auth/verify-email/resend`

Without a session. Request `{"email": "…"}`. Response **always** `202` with the same body as 20.2 —
for an unknown address, a finished account or an application already waiting for approval as well.

Errors: only `400 invalid_request`, `409 registration_disabled`, `415 unsupported_media_type`.

**No `429`**, and that is deliberate: a new token makes the old one invalid (20.9), at most **one
mail per five minutes per address** goes out, but the brake does not answer — above it the answer
stays `202` and only the mail does not go. A refusal here would be information that there is an
open application for this address.

### 20.5 `GET /api/v1/admin/registrations?limit=&offset=`

Permission: panel admin. Response `200` `RegistrationList`, form 1 from 1.8 as in 12.2,
`limit` ≤ 200 (default 50), newest first.

Carries `signup_ip`. That is the only trace by which five applications from one address can be
recognized; it is deleted at the admission (20.13) and has no business in a working account. It
carries **never** the token and **never** the password hash.

### 20.6 `POST /api/v1/admin/registrations/:id/approve`

Permission: panel admin. No body. Response `201` `PanelUser` — the same shape as 12.3, including
`system_user.state` and `error_message`.

Creates the `users` row and the system user (20.13), deletes the application row and sends
`account_approved`.

Errors: `404 registration_not_found`, `409 invalid_state` (the application is still unverified —
an address nobody has verified must not carry an account), `409 username_taken`,
`409 email_taken`.

### 20.7 `POST /api/v1/admin/registrations/:id/reject`

Permission: panel admin. Request `{"reason": "…"}` (may be left out). Response `204`.

Deletes the row, writes `registration_blocks` for **30 days** and sends `account_rejected` —
short, neutral, **with no reason**. The reason stays in the panel: a rejection with a reason is an
invitation to write something quotable, and the applicant can do nothing with the sentence.

Thirty days, because less brings the same applicant back into the list every week and more locks
out people who mean it. `until IS NULL` exists as well — that is the operator's block by hand, and
it is not set over this endpoint.

Errors: `404 registration_not_found`.

### 20.8 The four states, and what the sign-in makes of them

| State | where | system user | Sign-in (3.1) | what the person sees |
|---|---|---|---|---|
| no account | — | — | `401 invalid_credentials` | the form `/register` |
| `email_unverified` | `registrations` | no | `403 email_unverified` | "check your mailbox", button "resend mail" |
| `awaiting_approval` | `registrations` | no | `403 approval_pending` | `/registration-pending` |
| `active` | `users` | yes | normal | the server list |
| rejected | row gone, `registration_blocks` | — | as for unknown | the mail, neutral |

**The order is the contract** (3.1): the password first, the state second. If `users::by_name`
finds nothing, the sign-in asks `registrations` and checks the real hash **there** — that costs
exactly one argon2, as before. Only when nothing is there either does `verify_against_nobody()`
run (`api/session.rs:87`).

**No session for half accounts.** A `Caller` (`auth/extract.rs:19-60`) is a full account
throughout the panel; a second, weaker kind of session would have to be checked at every one of
the 138 method/path pairs, and one forgotten guard would be a user with a server without approval.
`must_change_password` is explicitly **not** borrowed for it.

### 20.9 The verification token

| Property | Decision | Reason |
|---|---|---|
| content | 256 random bits in base64url, 43 characters | like the session cookie, `auth/session.rs:159-181` |
| storage | SHA-256 only, `UNIQUE` index | like `sessions.token_hash`, `0002_schema.sql:52-53` |
| validity | **24 hours** | a verification grants no access, it only proves "this address exists", and people read mail in the evening. The reset link has a much shorter deadline for exactly this reason (21.5) |
| number | **one living one per application**; 20.4 devalues the old one | two valid links are two windows |
| maximum | five tokens per application | whoever cannot find five links needs another address |
| in the link | `<link_base>/verify-email#<token>` — in the **fragment** | a fragment reaches no server, so it lands in no access log (`main.rs` hangs `TraceLayer` over everything) and in no `Referer` — this panel loads Inter from Modrinth's CDN (`docs/INTERFACE-PARITY.md`, P5). The page accepts `?token=` as well and clears it out of the address bar at once with `router.replace` |
| redemption | a `POST` from the page (20.3) | the token is in no server URL (1.2) |
| in the log | never | the same holds for sessions in what exists today |

**Why a second click usually works anyway.** The token is used up once; whether a second
redemption can still answer depends on whether the row is still alive (20.3). A mail scanner does
not burn it: the link is a `GET` on a page of our interface, and the redemption is a `POST` that
only a person's browser sends. So a preview fetcher loads `index.html` and nothing else. **The
reset link must not copy this leniency** (21.5).

### 20.10 The address: normal form and uniqueness

Normal form: trim, **lowercase throughout**, exactly one `@`, local part 1–64 characters, domain
with at least one dot, no control characters, whole address ≤ 254 characters (RFC 5321).
Violations are `400 invalid_email`.

The local part is folded too, although RFC 5321 distinguishes it: every provider folds it, and
`Max@` and `max@` as two accounts would be a door to double accounts.

**No provider-specific normalization**: `max+1@gmail.com` and `max+2@gmail.com` stay two
addresses. For Gmail they are one mailbox, for many other providers they are not, and a rule that
throws other people's addresses together turns real people away. The consequence is named and is
caught elsewhere: approval on (the default), the daily cap, the block by hand.

Unique across two tables: `registrations.email UNIQUE`, `registrations.username UNIQUE` and a
`UNIQUE INDEX users_email` (several `NULL`s stay free, because accounts created by hand do not
have to have an address). The narrow race between "reserved" and "in `users`" ends **loudly** with
`409 username_taken` or `409 email_taken` at the admission, not quietly.

**Who asks across two tables depends on the question, and there are three different ones**
(`auth/users.rs:187-244`):

* **The admission (20.13) asks `users` only.** The application's row holds exactly the name the
  account is about to get; if it asked both tables, it would turn away every application.
* **The admin (12.3, 12.5) asks both.** An applicant who is reading their mail right now holds
  that name, and nobody may take it away from them while they do.
* **The sign-up form (20.2) asks both, with exactly one exception:** the unverified application
  for the address that stands in the form — it takes that one over whole. Without this exception,
  somebody whose verification mail went astray and who fills in the same form once more is told
  that their own name is taken. The way out of that would have been "resend" — that is, exactly
  the door the take-over shuts.

### 20.11 Brakes and no information

| What | Limit | Why this number |
|---|---|---|
| 20.2 per sender IP | 3 in 60 minutes, 10 a day | a household signs up once, behind NAT three times too |
| 20.3 per sender IP | 30 **unsuccessful** redemptions in 60 minutes, then `429 rate_limited` | a token cannot be guessed, but a guess must not be free either. Only the failures are counted, so that a second click by a real person — or a mail scanner that fetched the page — never spends the credit |
| 20.4 per address | 1 in 5 minutes, **silent** (the answer stays `202`) | without it the endpoint is a mail bomb; five minutes reads to a person as "it is on its way" |
| mails per address and kind | 5 in 60 minutes | the brake from 19.10, here without a count of its own |
| panel-wide per day | `daily_limit` from 19.2 | **explicitly no second counter**: the one in 19.10 sits in a table and survives a restart, a second one in memory would not |
| 21.1 per address and IP | see 21.6 | the same module, so that there are not two brakes |
| sign-in | unchanged (3.1) | holds for the two new 403s as well |

The brakes for 20.2 and 20.4 live in memory, like the sign-in brake and with the same admitted
price (`auth/brake.rs:5-7`): a restart forgets them. The panel-wide daily cap does not, and that
is exactly why it is the one that matters.

Two build decisions about these buckets that somebody would otherwise undo again
(`auth/rates.rs`): what is remembered are the **times** of the attempts and not a counter, because
`Retry-After` has to name seconds and only the oldest attempt in the window knows how many are
left. And the running panel's set of buckets is an `Arc` and not a `&'static`: process-wide is
intended, but two tests that share the same address would otherwise spend each other's credit and
fall over depending on the order. The key is cut to 64 characters before it is kept — what arrives
in the body is written by the caller, and a megabyte of it would otherwise sit in the table for a
day.

**The one line in `main.rs` without which none of these brakes counts.** Every brake from 20.11
and 21.6 hangs on the caller's address, and that address is only in a request when the router was
handed to `axum::serve` over `into_make_service_with_connect_info::<SocketAddr, _>`. Without it,
`ConnectInfo` is in no request, `from` is `None` in every handler, all three sign-in brakes count
nothing, and `signup_ip` — the only trace for looking something up later — stays empty. No test
that talks to its router with `oneshot` can see that: there is no connection there at all. It is
therefore guarded by a test that costs a real port, and by a text comparison against `main.rs`
(`registration/tests.rs:1238-1244`, `:1287-1290`; the list of house mistakes is in
`docs/WIRING.md`).

No information, three times over:

1. **Wording and length.** 20.2 and 20.4 answer `202` with the same body for every input, without
   `Set-Cookie` and without `Retry-After`.
2. **Effort.** argon2 always runs (20.2), for a known address and for a block as well.
3. **No name field.** 20.4 and 21.1 take an address only. "Username or e-mail" would be a name
   oracle for strangers.

### 20.12 Cleaning up

One task in the panel process, every six hours and once at startup, in the pattern of
`audit::spawn_purge` and `auth::disk::spawn_sweep`:

* `email_unverified` older than **7 days**: gone. The link is dead after 24 hours, seven days
  leave room for "at the weekend", after that name and address are free again.
* `awaiting_approval` older than **30 days**: gone. Whoever has not looked for 30 days is not
  going to look, and the operator is allowed to be on holiday.
* `registration_blocks` with `until <= now`: gone. `until IS NULL` stays.

**No block list for throwaway providers.** Lists like that are out of date on the day they ship
and hit real people. The approval switch is the answer, plus the block by hand.

### 20.13 What the admission creates

The admission is the **third door into the same corridor**: it calls `users::insert`
(`auth/users.rs:336`) and then `users::provision` (`:420`) — the same two functions as the CLI
(`auth/cli.rs:85-99`) and 12.3 (`api/admin.rs:283-297`). No new creation path.

* `role`: `user`, as a literal.
* `limits`: `default_limits` from 12.10 **at the moment of the admission**. There is no snapshot
  of the limits in the application.
* `must_change_password`: `false` — they chose their password themselves; `true` would send them
  straight to `/change-password` after the first sign-in.
* `email`: the address of the application. `origin`: `registration`.
* `signup_ip`: disappears with the row.

**The system user only comes into being here** and not when the form is filled in. The helper
calls `useradd`, writes `/etc/passwd`, `/etc/shadow`, `/etc/group` and creates `users/<id>/servers`
(`craftpanel-helper/src/users.rs:79-104`); every throwaway application would be a permanent entry
in `/etc/passwd`, a UID out of a finite pool and a directory, and cleaning up would need `userdel`.
A row in `registrations`, by contrast, costs nothing.

If the helper fails, the rule of 12.3 holds unchanged: the account is there, `system_user.state`
`"error"` with plain text, signing in yes, creating servers no
(`capabilities.blocked_reason = "system_user_not_ready"`), catching up over 12.9. No new error
path.

---

## 21. Forgotten password

The design is in `docs/PASSWORD-RESET.md`. Four endpoints, three of them without a session,
and one way that works without mail sending (21.9).

### 21.1 `POST /api/v1/auth/password-reset`

Without a session. Request `{"email": "…"}`. Response **always `202` with no body** — for a known
address, an unknown one, one without an account and one that belongs to an open application.

The call itself does exactly four things: read the body, ask the brake, count the brake up, hand
off the job. **Looking up, writing and sending lie in a detached task**, that is after the answer —
so there is no difference in time to measure (21.6). The existing code solves the same job at the
sign-in differently (the same amount of work, `auth/password.rs:89-93`), because a sign-in has to
return a result; here it does not.

Errors: `400 invalid_request` (field missing), `415 unsupported_media_type`,
`429 too_many_attempts`, `403 csrf_origin_mismatch`. **There is no `400 invalid_email` here**: a
check on the form would be information about the input, and an address without `@` simply finds no
account.

If no mail sending is set up (19.2), the answer stays `202` and **no row** comes into being. From
the outside this state cannot be recognized; whoever is allowed to know reads
`password_reset_enabled` from 20.1, and the page then shows, instead of the form, the sentence
"This panel has no password recovery set up. Ask your administrator."

### 21.2 `POST /api/v1/auth/password-reset/verify`

Without a session. Request `{"token": "…"}`. Response `200` `{"username": "max"}`.

The token travels in the **body**, not in the URL (1.2, 21.5). The name is the answer to "whose
password am I setting here anyway" — whoever has the token got the mail, and a form without that
much is an imposition. This answer **does not use up the token**.

Errors: `400 invalid_reset_token`, `429 too_many_attempts`.

### 21.3 `POST /api/v1/auth/password-reset/confirm`

Without a session. Request `{"token": "…", "new_password": "…"}`. Response `204`, **without
`Set-Cookie`**.

Five side effects, all in one write:

1. `users.password_hash` new, `must_change_password = 0` — they have chosen for themselves now,
   exactly as 3.4 does it (`api/session.rs:158`).
2. The token is used up (`used_at`), **all** remaining tokens of this account are deleted. The
   used one stays lying there until the sweeper takes it after 24 hours, and that is no oversight:
   if it were deleted along with them, `used_at` would be mere decoration, "works exactly once"
   would hang on the row being gone alone, and a later change to the cleanup would quietly make
   the link usable again. Besides, it is the only trace of who asked (21.5)
   (`auth/reset.rs:289-300`).
3. All sessions of the account fall (`session::close_all_of`, `auth/session.rs:106-113`); open
   WebSockets close with `4401`, because the socket checks its session again
   (`api/ws.rs:240-245`).
4. **No new session.** After the setting you are not signed in; the interface sends you to
   `/login?reset=done`. That is OWASP's advice, and it has a second use here: a missing approval
   (20.8) cannot be got around this way, because the way back leads through the sign-in.
5. `password_changed` goes out if an address is on file (`notify`, 19.14).

Errors: `400 invalid_reset_token`, `400 weak_password`, `429 too_many_attempts`.

A password that is too short does **not** count toward the brake. Otherwise somebody locks
themselves out because they typed something too short three times.

### 21.4 `POST /api/v1/admin/users/:user_id/password-reset`

Permission: panel admin. No body. Response `202`.

Here the answer may be **plain** — an admin knows that the account exists. Errors:
`404 user_not_found`, `409 no_email_address`, `409 mail_not_configured`, `409 mail_no_link_base`,
`429 too_many_attempts`, `429 mail_rate_limited`, `429 mail_quota_reached`.

The last two are the refusals of the mail area (19.10), and here they travel **outward**, unlike
in 21.1: the token is thrown away again and the admin learns the reason instead of standing in
front of a button that says `202` and does nothing.

**Everything that can refuse is asked before a row is touched.** First `configured()` (19.14) →
`mail_not_configured`, then `can_link()` → `mail_no_link_base`, then the cool-down of this way
(21.5) → `too_many_attempts`. Both mail questions separately, so that each gets its own sentence:
"no key" and "the panel does not know its own address" are two different things, and an admin
fixes them in two different places. The first build asked only `configured()` — with a key and
without a `link_base` it deleted the account's open token, minted a new one, could not build the
mail and then answered `409 mail_no_link_base`: one press on a button that can do nothing cost the
user the link they already had in their mailbox.

The endpoint is the extra with which an admin helps somebody without handing them a password that
a second person then knows (12.5).

### 21.5 The token

| Property | Decision | Reason |
|---|---|---|
| content | 256 random bits in base64url, 43 characters | like the session cookie, `auth/session.rs:159-181` |
| storage | SHA-256 hex only, `UNIQUE` index | like `sessions.token_hash`. No argon2: 256 real random bits are not a password, they cannot be guessed |
| validity | **30 minutes** | ASVS 5.0 6.5.5 demands at most ten minutes for out-of-band requests, but means push and OTP by it; a mail link has to survive delivery, a spam folder and a phone in a pocket. The house precedent for a deadline: `playit/mod.rs:50`, `CLAIM_DEADLINE = 15 min` |
| single use | yes, `used_at` | ASVS 6.5.5. **Unlike the verification token** (20.9): this one grants access to an account, that one only proves an address |
| number | one open one per account; a new request deletes the older ones | keeps the window small and makes "usable once" unambiguous; the mail says that only the newest link counts |
| cool-down 21.1 | one mail per 60 s, five per hour per account, above that silently discarded. **The hourly half does not take hold in what exists — 17.15** | keeps a stranger from filling a user's mailbox with our mails. Counted over the `created_at` of the rows, so it survives a restart |
| cool-down 21.4 | **no waiting time, but five links per hour per account** → `429 too_many_attempts`, checked **before** the old token is deleted. Its own counter (`rates::ADMIN_RESET_PER_ACCOUNT`), so independent of the brake that belongs to the user themselves | an admin who presses the button has just talked to the person — making them wait 60 s protects nobody, and whoever presses twice because the first mail did not arrive is right. It must not be unlimited all the same: every press is a mail in somebody else's mailbox **and** throws away this account's open link, and the second of those does not touch 19.10 at all, because it happens before the mail is sent. The upper limit is therefore the hourly number from 21.1: a stranger should not be able to send a user more links than the user can get themselves. The counter lies in memory; the restart-proof backstop is the mail brake from 19.10 (five per address and kind per hour, counted in `mail_outbox`) — but that is **no** backstop for the throwing away: it sits behind the deletion, and because it counts what the public form has already sent, it can refuse the admin's press after he has taken the living token. Measured in **17.15** |
| one code for three cases | `400 invalid_reset_token` for unknown, expired and used up | three codes would be an oracle about the state of other people's tokens. For the verification it is different (`404`/`410`), because there "expired" offers an action and the token grants no access |
| cleanup | used and expired rows older than 24 hours fall on every new request | the way the sign-in cleans up sessions (`api/session.rs:94`); no additional background task |
| in the link | `<link_base>/reset-password#<token>` — in the **fragment**, as in 20.9, and struck out of the address bar by the page at once with `history.replaceState` | a fragment reaches no server. A query string, by contrast, travels with the request for the page to *us*: in its default form nginx logs `$request` **and** `$http_referer`, so the token would stand in the access log of the proxy in front and after that in the `Referer` of every file the page loads (this panel loads Inter from Modrinth's CDN). `replaceState` comes too late for that — the request has long since gone out. The page keeps reading `?token=` (`web/src/pages/auth/recovery.ts:32-38`) so that a link from an older mail does not lead nowhere |
| in the log | never, in no `tracing` field and in no error message | |

### 21.6 No information, and the time is part of it

1. **Wording.** `202` with no body for every input. The interface always shows the same sentence:
   "If there is an account for this address, a mail is on its way."
2. **Headers and length.** No `Set-Cookie`, no `Retry-After`, no body — the answer is the same
   byte for byte. That is a test, not an intention.
3. **Time.** The handler *does* the same thing in both cases (21.1), instead of taking the same
   time. No `SELECT`, no `INSERT` with `fsync`, no network call in the request path. The way to
   check it needs no clock: in the test the outbox is held shut with a `Notify`, and the test
   demands that the `202` is already there while it is shut.
4. **Brake.** Ten attempts in 15 minutes, per address **and** per sender IP, then
   `429 too_many_attempts`. The build from `auth/brake.rs`, but an instance **of its own**: a
   shared one would mean that a storm of resets behind one NAT locks the *sign-in* of everybody
   behind it.

This refusal to give information can break quietly — if somebody later builds a check "does the
address exist at all" into the handler in order to answer `400` earlier, no test goes red except
the one from point 3. That one therefore has to be built along with the rest, not later.

### 21.7 Accounts with peculiarities

| State | Behavior | Reason |
|---|---|---|
| `must_change_password = 1` | allowed, and the flag falls | they have chosen for themselves now; afterwards the guard no longer sends them to `/change-password` |
| application `email_unverified` | **no mail, no token**, response `202` | there is no account. The address may well belong to a stranger whom somebody typed in while signing up; the way for that is 20.4 |
| application `awaiting_approval` | likewise | there is no account that could carry a password |
| account without an address | no mail | exactly the operator's case on the first day; their way is 21.9 — either `admin passwd`, or they add an address with `admin email` |
| `busy = 1` | is **not** checked | `busy` protects file system and UID work (12.6); here `users.password_hash` is written and sessions are deleted. A `409 user_busy` would be an error nobody understands, and from the outside it would give away that the account exists |
| `system_state = 'error'` | allowed | signing in works, creating servers does not (`auth/users.rs:288-301`); that has nothing to do with the password |
| panel admin | the same way as for everybody else | an admin without an address stays with 21.9 |

### 21.8 What else a password change devalues

**Every** password change discards all open reset tokens of the account: 3.4, 12.5, 21.3 and the
CLI from 21.9. Without this rule a link mailed long ago opens an account the owner has just taken
back, and that is the case where somebody changes their password *because* they suspect a
break-in.

**And every change of address**, for the same reason from the other side: the link lies in the old
mailbox, and whoever can change the address could otherwise redeem it after the change. It holds
for both doors, 12.5 and `craftpanel admin email` (21.9), and for deleting the address just as
much — a token nobody can learn about any more is a row waiting to be stolen (21.1). Sessions do
**not** fall with it: an address is not a means of signing in.

### 21.9 The two ways without mail sending — and the address itself

Both are subcommands next to `craftpanel admin create`, and the reason this section is of use on
the first day (task #25):

* **`craftpanel admin passwd --username max [--print-password | --password-stdin]`** — sets the
  hash, sets `must_change_password` exactly as the first creation does (`auth/cli.rs:55,92`: true
  with `--print-password`), throws away all sessions and all open tokens. Output as with `create`:
  the password on standard output, everything human on standard error.
* **`craftpanel admin reset-link --username max [--base-url https://panel.example.com] [--minutes 30]`**
  — mints a token and writes **the link** to standard output, in the same form as the mail:
  `<base>/reset-password#<token>` (21.5). With it the operator sends themselves the link, without
  a Resend key and without an interface. The basis is `link_base` (19.2); if it is missing,
  `--base-url` is required.

**No mail** from the CLI: there the operator is standing at the machine themselves.

Plus the command without which the two above stay the *only* ways:

* **`craftpanel admin email --username max [--address max@example.test | --remove]`** — sets,
  changes or removes the address of an account, with the same rules as 12.5: trimmed and
  lowercased (20.10), `email_taken` against `users` **and** open applications, and a change throws
  away the open tokens of the account (21.8). Nothing on standard output — there is nothing here
  for a pipe. On top of that, **`admin create --email …`** takes an address along right away, the
  way 12.3 takes one along.

Why a subcommand of its own and not a switch on `passwd`: changing an address has nothing to do
with the password, and `passwd` ends every session of the account. Whoever adds an address
afterwards should not have to sign anybody out for it, and the operator who has locked themselves
out needs exactly this way, because without a session they cannot get at 12.5.

---

## 22. Google Drive — backups in the user's Drive

The design with the measurements against Google is in `docs/DRIVE.md`. Twelve endpoints, and the
shape is playit's (18): **one account per panel user.** The operator sets up a Google project,
every user connects their own Google account, and the bytes go into **their** disk space, not onto
the operator's disk.

**Every call that reaches outside answers `409 external_services_disabled`** as long as the
panel-wide switch from 12.10 is off: 22.4 (the device flow starts at Google), 22.8 (`about.get`)
and 22.7 with `?files=delete` (deletes files in the Drive). Whatever only reads a row or clears
away a row of its own keeps answering — 22.3, 22.5, 22.6, 22.9 to 22.14 — because a switch that is
off must not take anybody's view of their own state away. From this follows the exception,
literally as in 18.5: `?files=keep` in 22.7 and 22.14 touches no foreign file and is therefore the
way out of the corner when Google cannot be reached. Handing the token back at `revoke` is a call
to the outside there too, but only as a best effort: it is skipped when the switch is off, and a
failure does not stop the disconnect — a token Google does not take back is one we throw away here
anyway.

**The secrets sit in files, not in the database** (the model is `0008_playit_per_user.sql`): the
client secret in `<data_dir>/drive/client_secret`, the refresh token per user in
`<data_dir>/drive/<user_id>/refresh_token`, both `0600` in `0700`. The access token lives only in
memory. The client id is in the database — it is no secret.

### 22.1 Why the device flow is the only way

Evidence retrieved 2026-08-13. The row of findings, in this order:

1. **A LAN address is doubly inadmissible as a redirect target.** "Redirect URIs must use the
   HTTPS scheme, not plain HTTP. Localhost URIs … are exempt" and "Hosts cannot be raw IP
   addresses" (developers.google.com/identity/protocols/oauth2/web-server).
   `http://192.168.1.10:8099/…` fails on both, and that is exactly what this panel looks like in
   most setups.
2. **The copy-it-across detour is dead.** "The manual copy/paste option, also referred to as an
   out of band (OOB) redirect method, is no longer supported"
   (developers.google.com/identity/protocols/oauth2/native-app).
3. **The detour over `127.0.0.1` does not carry here.** It presumes a listener on the machine in
   whose browser the consent is given. The user's browser is on their PC, the panel is on the
   server.
4. **The device flow covers exactly our scope.** "The OAuth 2.0 flow for devices is supported only
   for the following scopes", and the list contains
   `https://www.googleapis.com/auth/drive.file`
   (developers.google.com/identity/protocols/oauth2/limited-input-device). The same page: "Note
   that refresh tokens are always returned for devices."

**One way, no alternative.** No domain, no TLS, no redirect URI, no second flow "later, with a
domain". That is why this area stays small.

The mechanics, verbatim from the same page: `POST https://oauth2.googleapis.com/device/code` with
`client_id` and `scope`; answer `device_code`, `user_code`, **`verification_url`** (Google's field
name departs from RFC 8628), `expires_in`, `interval`. Polling over
`POST https://oauth2.googleapis.com/token` with `grant_type=urn:ietf:params:oauth:grant-type:device_code`;
errors `authorization_pending` (428), `slow_down` (403), `access_denied` (403), `expired_token`.

**The scope is `drive.file` and nothing above it.** It is enough: create a folder, files inside it,
read, change and delete our own files, and `about.get` for the storage level
(developers.google.com/workspace/drive/api/reference/rest/v3/about/get). `drive.appdata` would be
allowed as well but is rejected: the hidden app folder cannot be found by the user, and the
promise reads "the backup is in *your* Drive" — visible, clickable, downloadable by yourself.
`drive` and `drive.readonly` are "restricted" and would drag a review process with a security
assessment behind them.

### 22.2 What the operator sets up, and what "not set up" means

The admin page lists five steps with their addresses: create a project at
`console.cloud.google.com` → **enable the Drive API** (without it every call is
`403 accessNotConfigured`) → fill in the consent screen, audience *External*, scope
`…/auth/drive.file` → **"Publish app" → In production** → create a client of type **"TVs and
Limited Input devices"**, enter the id and the secret in 22.12.

**Step four is mandatory and has a warning of its own.** "A Google Cloud Platform project with an
OAuth consent screen configured for an external user type and a publishing status of *Testing* is
issued a refresh token expiring in **7 days**"
(developers.google.com/identity/protocols/oauth2). On "Testing" every connection therefore breaks
after a week — silently, and exactly when somebody needs a backup. The panel **cannot query** this
state; Google offers no interface for it. It can only say it and guess: if a refresh fails with
`invalid_grant` on a token younger than ten days, `last_error` says literally that this looks like
a consent screen on *Testing*.

Publishing needs **no** review process, because `drive.file` stands under "non-sensitive" in
Google's table (developers.google.com/workspace/drive/api/guides/api-specific-auth) and an app
with nothing but non-sensitive scopes does not go through the review
(developers.google.com/workspace/guides/configure-oauth-consent). Only whoever wants to see a name
and a logo in the consent screen gets as far as the light "brand verification" — work, not money.

**Nothing set up is a normal state, not an error.** Without a client secret: 22.3 answers `200`
with `panel_configured: false`, every attempt to connect is `409 drive_not_configured`, the
account page shows one sentence and no button, and the target `drive` appears in no picker.
Nothing starts, nothing calls, nothing crashes.

### 22.3 `GET /api/v1/drive`

Permission: signed in, and it is always your own state. Response `200` `DriveStatus`.

Reads one row and calls nobody. `panel_configured` is the question "has the operator set something
up", `configured` the question "has *this account* connected something". `configured: true` does
**not** say that it still works — the same honesty as `PlayitStatus.configured`
(`playit/mod.rs:103-104`); `state` is there for that.

**`state` is a statement about a connection, and `null` means there is none.** Three situations
fall under it — never connected, in the middle of it, last attempt went wrong — and none of them
is an error. The list of words deliberately has **no** word for them: what is running right now is
in `link` (exactly as playit puts the `claim` next to `agent.state`), and why the last attempt
failed is in `last_error` (22.5).

Measured on the running panel, and the reason for this paragraph: as long as the row was created
with `state: 'error'`, every account read `state: error, last_error: null` from the press on
"Connect" onward — the user was told that their account was broken, the operator the same about
every user, and both of them with no reason given. `0013_drive_account_state.sql` therefore makes
the column nullable and carries the two existing rows along: an `error` with no sentence and with
no check that ever ran was a stub and becomes `NULL`.

`link` is your **own** running process. It carries `user_code`, and that field is a secret of its
owner (22.11).

### 22.4 `POST /api/v1/drive/link`

Permission: signed in. No body. Response `201` `DriveLink` with `user_code`, `verification_url`,
`expires_at` and `interval`.

`201` and not `202`: the code **is** the answer and is complete at the moment it is handed over —
like 18.2. The panel holds at most four polling loops at a time (a semaphore, as with playit).

Errors: `409 drive_not_configured`, `409 drive_already_linked`, `429 upstream_rate_limited`,
`502 drive_unavailable`. The `message` of the `502` is the sentence from the table in 22.5 and not
Google's naked refusal: here of all places the refusals are usually the **operator's** (a client
of the wrong type, a wrong id), and "Google refused the call: Unauthorized" does not tell them
which of the five steps from 22.2 they left out.

The `device_code` is **not** in the database: it is the ticket the token can be collected with.
playit's `claim_code` is in there only because the *user* visits it. Consequence: a panel restart
throws a running process away and the user presses again. That is cheaper than the alternative.

### 22.5 `GET /api/v1/drive/link`

Permission: signed in. Response `200` `DriveLink`, otherwise `404 drive_link_not_found`.

The `404` means "no process open" — including after the one that worked. Whether it worked is said
by `configured` in 22.3 alone. Literally the shape of 18.3.

**Every abort names its reason, in Google's word and in ours.** `link.state` says *how* it ended,
`last_error` in 22.3 *why* — a sentence, not an identifier, and for the two refusals the
**operator** has to clear away themselves, with the page on which they do it. The mapping sits in
one place (`drive/oauth.rs::ending`), built like the mail table in 19.11: one row per answer of
theirs, one file each in `drive/testdata/`, one test each.

The names are Google's own, from their device flow page (step 6 "Other errors", step 2 for the
quota refusal; retrieved 2026-08-14), not invented:

| Google's answer | `link.state` | what the sentence says |
|---|---|---|
| `authorization_pending` (428) | — | no abort: the asking goes on |
| `slow_down` (403) | — | no abort: the interval doubles, as they demand |
| `access_denied` (403) | `denied` | refused — **or** the consent screen is on *Testing* and this account is not a test user; then Google refuses without asking. With the place: Google Cloud console → APIs & Services → OAuth consent screen → Audience, and there "Test users" or "Publish app" |
| `expired_token` (400, RFC 8628) | `expired` | the deadline ran out; if Google's page showed "An error occurred", it is the same *Testing* trap, with the same place |
| `admin_policy_enforced` (400) | `denied` | the Google Workspace administrator of this account has to release the client first |
| `org_internal` (403) | `denied` | the project admits only accounts of one Google Cloud organization; set the audience to *External* |
| `invalid_client` (401) | `expired` | id or secret wrong, or the client is not of the type "TVs and Limited Input devices" (22.2) |
| `invalid_grant` (400) | `expired` | this code is used up — fetch a new one. When *refreshing*, the same word means something else (22.2), which is why it stands here on its own |
| `unsupported_grant_type` (400) | `expired` | a fault of the panel, not a setting |
| `rate_limit_exceeded` (403, field **`error_code`**) | `expired` | too many codes for this client id; again later. This is the one answer in which Google names the field differently — whoever reads only `error` leaves it nameless |
| everything else | `expired` | Google's status and text, cut short — never an empty message |

A `waiting` that runs out on Google's deadline gets the same sentence as `expired_token`: from the
person's side that is one process and not two.

**A failed process does not set `state`.** It leaves nothing connected behind, so `state: null`
stays and only `last_error` is filled (22.3). A new process clears `last_error` away — whoever
tries once more does not see last time's complaint.

### 22.6 `DELETE /api/v1/drive/link`

Permission: signed in. Response `204`, otherwise `404 drive_link_not_found`.

### 22.7 `DELETE /api/v1/drive?files=delete|keep`

Permission: signed in. Response `204`.

The decision rides in the query so that no body hangs off a `DELETE` — the same shape as 12.6 and
18.5. Revokes at `https://oauth2.googleapis.com/revoke` and deletes the key file.

* `delete`: the archives in the Drive and their rows are gone.
* `keep`: the archives stay where they are, the rows stay with `drive_state: "unreachable"` —
  connecting again finds them over `appProperties` (22.17).

Without the parameter and with Drive backups present: `409 drive_has_backups`, so that the choice
is not made silently. Errors besides: `400 invalid_request` for any other word,
`409 drive_not_connected`, and with `?files=delete` the three translations of Google's refusals —
`429 upstream_rate_limited`, `502 drive_unavailable`, `507 drive_quota_exceeded`.

### 22.8 `POST /api/v1/drive/check`

Permission: signed in. No body. Response `202` `DriveStatus`.

Refreshes the token and calls `about.get`. The button "Check now", so that nobody waits an hour
for the sweeper (22.17). Errors: `409 drive_not_connected`, `429 upstream_rate_limited`,
`502 drive_unavailable`.

A **revoked** token is not an error here but a `202`: the state is in the row afterwards, the
answer carries it (`state: "revoked"`), and the page says the sentence. A `502` on top would be
the same message twice.

### 22.9 `GET /api/v1/servers/:id/backups/target`

Permission: `BASE_READ`. Response `200` `BackupTarget`.

`target` is what is set, `effective_target` is what the next run really takes, and `reason` says
**why** a target cannot be chosen: the operator's rule, no connected Drive, or nothing set up.
Without these three fields the interface would have to guess why a toggle is gray.

**`not_connected` means "no Drive a run could put anything into"**, and that includes a
**revoked** access (`state: "revoked"`, 22.3). The key file stays lying there on a revocation,
because the state belongs in the column; its mere presence would therefore keep saying "connected"
while every upload failed. `error` explicitly does **not** belong to it: a sweeper that once did
not reach Google is no reason to refuse a backup that could succeed.

This field is therefore also the only way an **editor** learns about a broken access: they may
know nothing about the owner's Google account except the one fact that a backup of this server
currently has nowhere to go.

**`reason: "policy"` with `effective_target: "drive"` is the healthy state** (`drive_only`, Drive
connected). Whoever makes a warning out of it — "`reason !== 'ok'`" — locks every button on a
`drive_only` panel, even when everything carries. The interface therefore reads exactly the two
words 10.2 answers with a refusal: `not_configured` and `not_connected`.

### 22.10 `PUT /api/v1/servers/:id/backups/target`

Permission: `BACKUPS`. Request `{"target": "local" | "drive"}`. Response `200` `BackupTarget`.

**Moves no bytes** and applies from the next run on; existing rows keep their `location` value
forever (10.1).

Errors: `400 invalid_request`, `409 drive_not_configured`, `409 drive_not_connected` (the
**owner** has not connected one — the target is always their Drive, even when an editor switches
over), `409 target_not_allowed`.

The operator's rule is `target_policy`, panel-wide, with three values and the default
**`user_choice`**:

| Value | means |
|---|---|
| `user_choice` | one target per server, default `local`. The user switches over as soon as they are connected |
| `drive_only` | "Drive **instead of** my server": **a user without a connected Drive cannot make backups.** 10.2 answers `409 drive_not_connected`, the backups page shows the way to the account page instead of the button, and a scheduled run ends with `last_status: "failed"`. That is hard and has to look hard, otherwise somebody believes they are backed up |
| `local_only` | the off switch for when Google makes trouble. Existing Drive backups stay readable and restorable; only new ones go local |

**What "shows the way instead of the button" means, and it holds for every trigger on this page.**
If a backup cannot succeed — `effective_target: "drive"` with `reason` `not_connected` or
`not_configured`, then the trigger is **locked** and the reason stands next to it as a
**sentence**, not as a tooltip: on a phone there are none. That affects "create backup", the retry
of a failed one (the same thing again) and **switching on** the schedule (10.10) — switching off
stays possible always, otherwise nobody would get out of a schedule that fails every night.

For the **owner** the sentence comes with a way: a button to their account page, where 22.4 is
waiting. If the panel's Google project is missing (`not_configured`), even they can do nothing,
and the explanation stays without a way. A stranger never gets a way: it is not their account —
literally the shape 18.8 has for playit (`components/PlayitAddress.vue`).

The open button next to its own explanation was measured reality ("Backups of this server cannot
be made right now" and next to it an active "Create backup" that brought
`409 drive_not_connected`); the chain up to the connected Drive has a guard in
`web/src/pages/servers/backup-drive-path.test.ts`.

**Local stays possible**, and that is a product decision with three reasons: today there is
neither a Google project nor a client secret, so a panel without a Drive could not make backups at
all; the free Google storage is 15 GB and shared with Gmail and Photos (by reported accounts, new
accounts have had only 5 GB since March 2026, and 15 only with a phone number on file), while the
state of a modpack is 2–5 GB; and existing backups must not disappear.

### 22.11 `GET /api/v1/admin/drive`

Permission: panel admin. Response `200` `DriveAdminOverview` — the settings plus one row per user
with a connection, sorted by username.

The row comes into being as soon as somebody presses "Connect" (22.4), and then carries
`state: null`: **nothing connected**, no error (22.3). For that the interface writes "Nothing
connected" and not "Not working", and the reason for a real failure stands next to it in
`last_error` — that is everything the operator needs in order to tell whether they, Google or the
panel is to blame.

**No `link_user_code`, in no row.** Whoever confirms somebody else's code hangs **their own** Drive
onto **somebody else's** panel account; from then on that account's backups flow into the
stranger's Drive. That is data flowing out, not merely bad manners — the same reasoning with which
18.10 leaves out the `claim`. A test that taps every admin answer for this field is cheap and
prevents exactly the mistake you make when you extend a struct.

No field that could carry the client secret or a token.

### 22.12 `PUT /api/v1/admin/drive`

Permission: panel admin. Request `UpdateDriveSettingsRequest`, response `200`
`DriveAdminOverview`.

One endpoint for everything panel-wide in this area — client id, client secret, `target_policy`
and `folder_name` — in the shape of 19.3: `client_secret` left out or `null` means **unchanged**,
`""` means **delete**, a text means **replace**. Without these three meanings, every change to the
rule would delete the secret.

The id goes into the row, the secret into the file (`0600` in `0700`, written as in
`playit/agent.rs:345-359`: `.part`, `sync_all`, `rename`) and **never** comes out again. Errors:
`400 invalid_request`.

These four values are explicitly **not** in `panel_settings`: there 12.11 writes the row as a
whole, and two areas in one row would mean two hands on `auth/settings.rs`, `api/admin.rs` and the
settings page. Besides, this way no body of `PUT /admin/settings` can ever carry a secret.

### 22.13 `DELETE /api/v1/admin/drive/credentials`

Permission: panel admin. Response `204`. Afterwards the panel is "not set up" again.

Existing connections of the users stay standing as rows but no longer work — at the next sweep
they become `state: "error"`. The confirmation dialog says so, literally.

### 22.14 `DELETE /api/v1/admin/drive/:user_id`

Permission: panel admin. Response `204`. Errors as in 22.7, plus `404 user_not_found`.

**No `?files=delete`.** A panel admin has nothing to delete in another person's Drive; this
endpoint disconnects and leaves everything lying there, always `keep`. The difference from 18.11
(`?tunnels=`) has a reason: there a port on somebody else's account would otherwise stay taken
forever, here there are only files in somebody else's storage, which their owner sees and can
throw away themselves.

Writes a `warn!` line with the actor and the target; an audit entry after 11.9 is impossible
(17.9), the same gap as with 18.11.

### 22.15 The course of a backup into the Drive

**Build → upload → delete locally**, in that order, and the first reason settles it:

`quiesce::Held::take` switches `save-off`, `Drop` switches it back, and today the bracket encloses
exactly the packing (`crates/craftpanel/src/backups/mod.rs:671-712`). `quiesce.rs:5-8` names "the
whole risk of this area: a server left with saving switched off looks perfectly healthy and loses
everything since the last flush the moment it crashes." **A streaming upload would hold `save-off`
for the whole duration of the upload** — with 2 GB and 10 Mbit/s that is half an hour without
saving. That alone decides the question. The upload lies **outside** the bracket, and that belongs
at the place as a comment with its reason; a test holds it down.

Two further reasons say the same: a `tar`+zstd stream is not reproducible (`archive.rs` has tests
of its own for the case "the file shrinks while it is read"), but a resume demands the same bytes
at the same place; and the finished file knows its length, so `X-Upload-Content-Length` is set and
`Content-Range` is exact.

One run, one operation (`backup_create`), three sections: packing as today (progress 0 → 0.5),
uploading (0.5 → 1.0), then **write the row first, delete the local file second** — the other way
round, a crash in between would be a file without a row *and* without a local copy (the same
ordering argument as `forget()`, `backups/mod.rs:435-437`). **No new `OperationPhase` value**
(5.9).

The upload (developers.google.com/workspace/drive/api/guides/manage-uploads):
`POST …/upload/drive/v3/files?uploadType=resumable` with the metadata in the body, the session URI
in the `Location` header, valid for **one week**; chunks as `PUT` in multiples of 256 KiB, chosen
**8 MiB**; `308` means carry on, and the `Range` header of the answer says what arrived — **the
answer may carry a new `Location`, and then that one counts**; the state after an abort over an
empty `PUT` with `Content-Range: bytes */<total>`; `404` means "session expired, start over";
`5xx` and `429` with `min(2^n + jitter, 64 s)`
(developers.google.com/workspace/drive/api/guides/limits), five attempts per chunk.

Three limits must **not** get into the backoff cycle, otherwise the upload hammers for a minute
against a wall that only opens tomorrow: 750 GB of upload per user account per day, 5 TB per file,
and `storageQuotaExceeded` — according to Google that one is explicitly not to be retried, and it
becomes `507 drive_quota_exceeded` or the operation error `drive_quota_exceeded`.

**Canceling** (5.4) checks the same `archive::Progress::is_cancelled` between two chunks. The
consolation: as long as a resumable session is not finished, **no file appears in the user's
Drive** — an aborted run leaves nothing lying there, and an unfinished session expires by itself
after a week. That is why **nothing has to be added** for 5.12 (restart): `recover()` throws the
half archive away, and there was never anything to see in the Drive.

File name `<server-slug>--<backup-slug>--<created_at>.tar.zst` (`slug()` already exists,
`api/backups.rs:300`), plus `appProperties { panel: "craftpanel", server_id, backup_id }` — the
limits are 30 private properties per app and 124 bytes per property, and three ULIDs fit into that
twenty times over.

### 22.16 Restoring from the Drive

Download, then the existing `unroll` — **no second restore path**.

1. `files.get?fields=size,md5Checksum,trashed` — is it there, and how big.
2. **Check the space before anything runs**: free space ≥ (archive size + estimated unpacked size)
   × 1.1, plus the owner's disk limit. A finding on the side, so that it does not pass as a silent
   rebuild: today `unroll` checks **none** of this (`backups/mod.rs:780-851`) — with a local
   backup the archive was at least there already, on the Drive path it has to get there first.
3. `GET …/files/<id>?alt=media` into a `.part` file (progress 0 → 0.4).
4. **`md5Checksum` against what was computed locally** (`md-5` is already in the `Cargo.toml`).
   Half a download is half a server; here a checksum is no ornament.
5. Rename, `unroll` unchanged (0.4 → 1.0), delete the local copy at the end.

Errors of the operation: `drive_file_missing`, `drive_revoked`, `drive_unavailable`, `no_space`,
`disk_limit_reached` (5.11).

### 22.17 The sweeper and `drive_state`

The user can delete, rename and move things in their Drive. Renaming and moving make no
difference (we hold the id). Deleting and the trash do.

**One sweeper per connected user, once an hour**, spread out like playit's sync
(`playit/connection.rs:806`: `spread % SYNC_TICK`, so that they do not all start calling at the
same time on startup). Three calls: refresh the token (which is at the same time the check for
`invalid_grant`), `about.get` for the storage level, and **one** `files.list` with
`q = "appProperties has { key='panel' and value='craftpanel' } and trashed=false"` for all backups
of this user.

| Situation | `drive_state` | Consequence |
|---|---|---|
| file there | `present` | nothing |
| file in the trash | `trashed` | the list shows it; restoring is refused with the note to get it back in the Drive |
| file missing | `missing` | the list shows it; `409 backup_not_restorable` with plain text; deleting in the panel stays allowed |
| connection cut with `keep` | `unreachable` | the row stays, connecting again finds the file over `appProperties` |
| file there whose `backup_id` belongs to no row with exactly this `drive_file_id` | — (orphan) | is deleted, with an `info!` line |

The orphan rule covers the two cases in which something can be left over: a crash after the last
chunk, and a retry after 10.7 that creates a new file. **This rule has to stay narrow** — one that
is too wide deletes something in a person's storage that belongs to them. A test with a foreign
file in the same folder that must *not* be touched belongs with it.

A revocation (`state: "revoked"`) hangs in three nets, so that it is not noticed only when a
backup is made: the sweeper (one hour), an `Admonition type="critical"` on **every** backups page
of a server with the target `drive`, and `should_prompt: true` for failed automatic backups
(10.1). The fourth net is a mail — the template for it is not among those in 19.12 and would be a
ninth; the hook for it sits in the sweeper and today only writes `warn!`.

### 22.18 Quota, disk limit, schedule

* **The count quota (10.12) counts Drive backups too.** If they did not count, "50 local and 50 in
  the Drive" would be a way around 10.12.
* **The disk limit (12.7) does not count them.** `auth/disk.rs` sums `backups.size_bytes` over all
  servers of the account; without an `AND b.location = 'local'`, a backup of which not one byte
  lies here would weigh on the pot forever. `size_bytes` stays set (10.1 shows the size), only the
  sum leaves Drive rows out. **That is the one line where a bug gets built in if nobody looks.**
* **On the way it counts all the same** (22.15, 10.2), and that is right: the archive really is on
  the disk. After the run it is gone. A limit of the honesty that you have to know: `Disks`
  remembers for 60 seconds (`auth/disk.rs:31`), so a file that has just been deleted still counts
  for up to a minute afterwards. That is already the case today.
* **Schedules (10.10) carry unchanged.** `skipped_unchanged` compares mtimes in the server tree,
  not the target. Cleaning up still happens only among automatic backups; for a Drive row,
  cleaning up means `files.delete` and then the row — the row first, the file after, as in
  `forget()`. If `files.delete` fails, the file stays standing in the user's Drive, with a `warn!`
  line: it lies in *their* storage, they see it and can throw it away. That is no loss of data,
  and the alternative — keeping rows that nobody clears up any more — is worse.

### 22.19 Downloading: a link, no pass-through

`Backup.drive_web_link` is `https://drive.google.com/file/d/<id>/view`. The file belongs to the
user, they are signed in at Google, and **the panel transfers no byte**. 10.8 and 10.11 answer
`409 backup_lives_in_drive` with the link in the `message`; our own row next to the borrowed
`BackupItem` becomes the button "Open in Drive" — the same place the replacement download already
hangs on today.

A pass-through would be more convenient and would cost exactly the bandwidth this whole section is
meant to save, **twice** (down and up). It is not built. Besides, the link route gets around the
known limitation of 10.11 that `BackupItem.vue:116` writes `https://` into its URL as a constant.

### 22.20 What is explicitly not built

* **No encryption of the archives.** A backup contains the whole server tree, and therefore
  `server.properties` with the RCON password and plugin credentials as well; unencrypted, Google
  can read every backup. Encrypted, the file in the Drive would no longer be usable with one
  click, and a forgotten password is a lost server. **The operator has to know this**, and the
  account page says it in one sentence — it is the only point of this section where a consent
  gives anything in substance.
* **No deleting in a stranger's Drive by an admin** (22.14).
* **No pass-through** (22.19), **no `drive.appdata`**, **no domain administration**, **no second
  provider** and **no subfolder per server** (one folder per user, one call for the sweeper; the
  file name carries the server name).
* **Costs: none.** A Google Cloud project, the Drive API and the device flow are free at this
  scale. Payment happens only when a *user* buys more storage — their money, not the operator's,
  and exactly the point of all this.
