# Wiring

As of 2026-08-13. One kind of fault was hunted: **built, compiled, tested — and nobody calls
it.** A green test proves that a function works, not that it ever starts.

Checked against the running service on `127.0.0.1:8099`, with the source read alongside it. Every
finding carries its evidence: the call and the answer, the log line or the row in the database.

## The numbers

| | |
|---|---|
| Router functions (public, production code) | 18 |
| of those, mounted in `main.rs` | 14 |
| Route registrations in production code | 82 (106 method/path pairs) |
| of those, answered live | 105 of 106 |
| Endpoints in `web/src/api/client.ts` | 100 entries, 93 pairs |
| Client paths with no route behind them | 1 |
| Client functions no interface calls | 6 |
| Background tasks that ought to start once | 12 |
| of those, started | 6 |
| Provider fields in Modrinth's contracts | 253 |
| of those, filled | 214 |
| Public functions in production code | 728 |
| of those, with no caller outside their own file | 43 |

**8 BROKEN · 12 DEAD · 36 HARMLESS**

---

# BROKEN — the user notices

## K1 · Every restart of the panel kills every running server

`Manager::spawn_recovery()` (`crates/craftpanel/src/servers/manager.rs:1439`) has **no caller**.
It is the only place that calls `adopt_tokens()`, and `adopt_tokens()` (`manager.rs:1342`) is the
only place that calls `Hub::load_tokens()` (`servers/hub.rs:79`). The chain leads back to nobody.

`main.rs:151` instead calls only `manager.reconcile().await`, without tokens and without the
grace period. The comment above `reconcile` says so itself:

> Run this after the grace period of `REATTACH_GRACE`, or a supervisor on its way back is
> declared dead two seconds before it knocks.

The consequence, in this order:

1. `hub.tokens` is empty after startup.
2. A supervisor that is still alive and reports back is rejected: `hub.rs:151`,
   `rejected a supervisor claiming to be …`.
3. `reconcile` sees `attached = 0`, writes the server to `stopped`, sets
   `supervisor_token = NULL` (`manager.rs:1376`) and calls `forget_token`.

With that the server is orphaned for good: the row says "stopped", the secret is deleted, and
the running process can never be attached again. The very property the supervisor exists for is
gone.

**Evidence**: three restarts in the log, the same picture every time, eleven to twenty-two
milliseconds after startup:

```
2026-08-13T01:20:56+02:00  INFO craftpanel: starting path=/etc/craftpanel/config.toml
2026-08-13T01:20:56+02:00  INFO craftpanel::servers::manager: servers reconciled attached=0 cleared=1 broken=0 resumed=0
2026-08-13T03:37:37+02:00  INFO craftpanel::servers::manager: servers reconciled attached=0 cleared=1 broken=0 resumed=0
2026-08-13T03:48:27+02:00  INFO craftpanel::servers::manager: servers reconciled attached=0 cleared=1 broken=0 resumed=0
```

and the rejection, several times:

```
2026-08-13T03:53:07+02:00  WARN craftpanel::servers::hub: supervisor connection ended:
                                rejected a supervisor claiming to be 01KZW481FX4TCZAMD9JHEHJ0JG
```

`attached=0` on a running machine is the proof. `cleared=1` is the damage.

## K2 · The readouts for CPU, memory and disk stay empty forever

`Manager::spawn_metrics()` (`servers/manager.rs:1501`) has **no caller**. It is the only place in
production code that calls `channel.stats(sample)` (`manager.rs:1532`). Without it not one
`stats` frame goes over the socket. Section 13.4 wants one per second per running server.

`api/ws.rs:280` only replays the ring buffer on connect, which is empty, because nobody ever
fills it. So not even a zero arrives.

**Evidence**: 75 seconds on the socket of a *running* Paper server:

```
2026-08-13T02:00:16.760Z state {"type":"state","power_state":"running", …}
COUNTS {"server":1,"state":3,"operations":1,"console_history_start":1,"console_history_end":1,"console":26}
STATS_FRAMES 0
```

26 console lines arrived, three state changes arrived, **zero** measurements. The tiles on every
server page stay empty, with no error message: exactly the silent empty area.

## K3 · Automatic backups never run

`Backups::spawn_scheduler()` (`backups/schedule.rs:206`) has **no caller**. It is the only place
that calls `tick()`; `tick` is the only place that calls `run_scheduled`; `run_scheduled` is the
only place that reaches `store::record_run` (`backups/store.rs:477`) and `prune()`. Four links,
no beginning.

The interface accepts the schedule and shows it as switched on; it just never fires, and
`keep_last` never cleans up.

**Evidence**:

```
PUT /api/v1/servers/…/backups/schedule   {"enabled":true,"interval_hours":1,"hour_utc":0,"keep_last":3}
200 {"enabled":true,"interval_hours":1,"hour_utc":0,"keep_last":3,
     "next_run_at":"2026-08-13T03:02:40Z","last_run_at":null,"last_status":null,"last_error":null}
```

`next_run_at` is set, `last_run_at` stays `null` forever, because nobody reads `store::due`.
`TICK` is set to 60 seconds and does not tick.

## K4 · The three watchdogs from section 5 and the line counter from 13.5 are asleep

`Operations::spawn_housekeeping()` (`ops/mod.rs:353`) has **no caller**. It is the only place
that calls `housekeeping()`. That takes out four things at once:

| What fails | Contract | Consequence |
|---|---|---|
| `store::sweep_timeouts` with `STALL_LIMIT` (10 min) | 5.11 | a stuck operation stays `ongoing` forever and blocks 5.13 permanently |
| the same with `PAYLOAD_LIMIT` (15 min) | 5.7 | a run waiting for a follow-up delivery waits forever |
| `purge()` | 5.12 | finished operations are never cleared away |
| `persist_console_counts()` | 13.5 | the line counter that must never jump back jumps to 0 on every restart |
| `sweep_work_dirs()` | — | work directories are left lying around |

**Evidence** for 13.5: `ops/mod.rs:371` is the only writer of `console_seq`, and after four
servers with console output the database says:

```
('01KZW3YSBRPG25BSMZ52WMCQR5', 'Probe',           'available', 0, …)
('01KZW481FX4TCZAMD9JHEHJ0JG', 'test',            'available', 0, …)
('01KZWC9ZJSMK3H408DBPGXQJDB', 'VanillaProbe',    'available', 0, …)
('01KZWDG8W32A9FX2S6NQMXJ90A', 'wireaudit-probe', 'available', 0, …)
```

Four times `console_seq = 0`.

## K5 · The download button on a backup leads to the panel's start page

`api::backups::compat_router()` (`api/backups.rs:88`) is fully built, has four tests of its own,
and is **not merged** in `main.rs`. It is the only path outside `/api/v1` (10.11), it belongs at
the root, and nothing hangs there.

**Evidence**:

```
GET http://127.0.0.1:8099/modrinth/v0/backups/01ARZ3NDEKTSV4RRFFQ69G5FAV/download
200  content-type: text/html
<!doctype html><html lang="en">	<head>		<meta charset="UTF-8" /> …
```

200 with `text/html` means: the SPA fallback route from `web.rs:16` answered, not the handler.
For comparison, a route that really does not exist:

```
GET /api/v1/definitely-not-a-route      404  text/plain  "not found"
```

Who it hits: `BackupItem.vue:116` from Modrinth's library builds the link itself and writes
`https://` into it for good. `Backups.vue:423` therefore sets `downloadHost` only on a panel with
TLS, and with `v-if="downloadHost === undefined"` hides its own replacement button exactly then.
On `http` it is intact, then; on `https` the user clicks "Download" and gets the panel's start
page as a file. The contract filler `api.backups.aliasDownloadUrl` (`client.ts:963`) exists for
this path and nobody calls it.

## K6 · Nobody can accept an invitation

Three endpoints are built and answer:

```
GET  /api/v1/invitations                        200  {"invitations":[]}
POST /api/v1/invitations/01ARZ…/accept          404  {"error":"invitation_not_found"}
POST /api/v1/invitations/01ARZ…/decline         404  {"error":"invitation_not_found"}
```

`404 invitation_not_found` instead of `404 not found` in `text/plain` means: the route is there,
only the invitation is not. All three have their function in `client.ts`
(`api.access.invitations`, `acceptInvitation`, `declineInvitation`).

In `web/src` outside `client.ts` and `types.ts`: **zero hits** for `invitations`,
`acceptInvitation`, `declineInvitation`. No inbox, no route in `router.ts`, no page, no badge in
the header bar.

That makes section 11 a dead end: `POST /servers/{id}/members` creates an invitation the invitee
sees nowhere. In `Access.vue` the inviter sees an entry "invited" that never changes.

## K7 · Modpacks cannot be reached from the interface

`client.content.installModpack` (`client.ts:730`) has **zero callers** in `web/src`. It is the
only door through which a modpack gets onto an existing server.

The second door would be the create wizard. But `New.vue:810` sends `kind: 'loader'` and nothing
else; `modpack_project` and `modpack_upload` from `CreateServerContent` (`types.ts:236`) are
created nowhere.

The third door would be the browse tab. `browse-manager.ts:265` sets
`projectType = list.value?.content_type ?? 'mod'`: that is the loader's content type
(`mod`, `plugin`, `datapack`), never `modpack`.

What that puts out of reach, although it is built, wired and tested:

- `POST /servers/{s}/content/modpack/install` — answers live (`404 server_not_found`)
- `GET  /servers/{s}/content/modpack/contents`, `…/modpack/update`, `…/modpack/unlink`
- `PUT  /servers/{s}/operations/{op}/payload` (5.7) — `client.ts` has `putPayload`, zero callers
- `content/mrpack.rs`, the modpack card in `content-manager.ts:606`, `:574`, `:616`

Side effect with K4: a run waiting for a follow-up delivery is not cleared away by the watchdog
either. So it could not only never be served, it could also never be finished.

## K8 · A long download halts every other piece of work in the whole panel

`Manager::spawn_dispatcher()` (`servers/manager.rs:616`) has **no caller**. It was built to give
each run its own task; its comment says why:

> Each run gets its own task: 5.13 allows more than one at a time, and a download that takes
> a minute may not hold up a delete.

Instead, `run_operations` in `main.rs:262` works the queue **serially**:

```rust
for id in queued {
    if manager.run(id).await { continue; }
    backups.run(id).await;
}
```

`manager.run(id).await` waits out the whole run. While a Paper server downloads 51 MB, nothing
else starts in these 500-millisecond rounds — no backup, no delete, no second server, on no
machine. The user sees several operations sitting at `queued` with nothing moving. Functionally
correct, but against 5.13.

---

# DEAD — built, unused, does no harm today

| # | What | Where | Why it does not hurt |
|---|---|---|---|
| ~~T1~~ | ~~`Content::sweep_updates` and the chain `sweep_once` → `check_updates_gently`~~ | `content/mod.rs:774` | **resolved** — `main.rs:175` calls `content.sweep_updates(live.clone())`; the chain is alive |
| T2 | `api::router()`, `api::admin::router()`, `api::session::router()` | `api/mod.rs:26`, `admin.rs:34`, `session.rs:26` | the `with_live` siblings are mounted; these three are only the test variants without a hub |
| T3 | `api.operations.putPayload` | `client.ts:496` | the route is there, the state that would need it is out of reach (K7) |
| T4 | `api.backups.aliasDownloadUrl` | `client.ts:963` | the route is not mounted (K5) |
| T5 | `auth::extract::session_ref` | `auth/extract.rs` | zero references, not even in tests |
| T6 | `Backups::used_quota` | `backups/mod.rs` | zero references, not even in tests |
| T7 | `Modrinth::project_is_fresh` | `content/modrinth.rs` | zero references, not even in tests |
| T8 | `Channel::seconds_since` | `ops/events.rs` | zero references, not even in tests |
| T9 | `UserProfileContext`, 12 fields, none filled | `vendor/…/user-profile/providers/user-profile.ts` | the interface never embeds `UserProfilePageLayout`; the panel has no profile page |
| T10 | 27 optional provider fields in embedded contracts | see section 4 | all guarded in the vendor with `?.` or `v-if` — the button is missing, nothing breaks |
| T11 | `ServerSettingsContext.closeModal` | `pages/servers/Settings.vue:90` | only the modal variant needs it, the panel uses the page variant |
| T12 | `pnpm --filter @craftpanel/web build` checks no types | `web/package.json:9` | `build` is `vite build`; `typecheck` (`vue-tsc --noEmit`) is a script of its own and part of no workflow. Today there are **0** type errors in `web/src` — but the build would not notice a missing required field in a contract |

---

# HARMLESS

- **32 public functions** are helpers called only inside their own file or only from tests:
  `has_room_for`, `is_unlimited`, `check_strength`,
  `memory_total_mib`, `admin_count`, `is_cancelled`, `take_within`, `with_analyst`,
  `effective`, `aliases`, `normalise_loader`, `newest_fitting`, `wanted_on_a_server`,
  `looks_like_a_bomb`, `over_the_ceiling`, `with_http`, `allows_running_server`, `union`,
  `is_panel_admin`, `insert_in`, `ensure_binary`, `secret_path`, `create_body`, `confined`,
  `discover`, `managed_flags`, `record_by`, `is_modpack` and siblings.
- **5 functions are dead only indirectly** and come alive as soon as K1–K4 are wired:
  `adopt_tokens`, `housekeeping`, `sweep_work_dirs`, `prune`, `sweep_once`.
- **9 route registrations** sit in test stubs and do not belong in the count:
  eight in `content/harness.rs:190-197` (the fake Modrinth API), one in `api/console.rs:402`
  (`/1/analyse`, the fake mclogs API).
- **710 type errors** reported by `vue-tsc`, all in `vendor/` and `node_modules/`, none in
  `web/src`.
- No interface calls `GET /api/v1/health`. That is deliberate.
- **`#![allow(dead_code)]` sits at the head of nine area modules**, and the reason is exactly the
  kind of fault this report is about: as long as `main.rs` does not mount a router or does not
  build a service, the whole area looks unused. So the line is the note "something here still
  hangs on `main.rs`" and **not** a silent tolerance of dead code; it has to go as soon as an
  area is fully wired. Affected:
  `api/mod.rs:6`, `audit/mod.rs:23`, `backups/mod.rs:20`, `content/mod.rs:14`, `drive/mod.rs:23`,
  `files/mod.rs:17`, `mail/mod.rs:29`, `playit/mod.rs:16`, `settings/mod.rs:10`, plus the two
  `#[allow(unused_imports)]` on the two `pub use` in `audit/mod.rs:30,32` (the event catalog from
  11.9 is wider than its writers, because the remaining areas bring their entries along with
  their own endpoints).

---

# 1 · Routers

18 public functions in production code return an `axum::Router`, plus three private `routes()`
helpers and four test stubs.

`main.rs:180-197` mounts twelve places that cover fourteen of these functions (`api::with_live`
pulls `session::with_live` and `admin::with_live` in with it). Every mounted line exists, none
points into the void: the service builds and runs.

Not mounted:

| Function | Classification |
|---|---|
| `api::backups::compat_router` (`api/backups.rs:88`) | **BROKEN** — K5 |
| `api::router` (`api/mod.rs:26`) | harmless, test variant |
| `api::admin::router` (`api/admin.rs:34`) | harmless, test variant |
| `api::session::router` (`api/session.rs:26`) | harmless, test variant |

**Evidence for "connected"**: one real call per router, none of them 404-with-`text/plain`:

```
GET  /api/v1/admin/host                       200  {"cpu_cores":16,"memory_total_bytes":25769803776,…}
GET  /api/v1/me                               200  {"id":"01KZWD77F5KW5GE6YN6V8DXH76",…}
GET  /api/v1/servers                          200  {"servers":[],"users":{}}
GET  /api/v1/operations                       200  {"operations":[],"busy_reasons_by_server":{}}
GET  /api/v1/servers/{s}/ws                   400  Connection header did not include 'upgrade'
GET  /api/v1/servers/{s}/files/meta           404  {"error":"server_not_found"}
GET  /api/v1/servers/{s}/content              404  {"error":"server_not_found"}
GET  /api/v1/loaders                          200  {"loaders":[{"id":"vanilla",…}]}
GET  /api/v1/servers/{s}/backups              404  {"error":"server_not_found"}
GET  /api/v1/invitations                      200  {"invitations":[]}
GET  /api/v1/admin/playit                     200  {"configured":false,…}
GET  /api/v1/servers/{s}/console/logs         404  {"error":"server_not_found"}
GET  /api/v1/modrinth/v2/tag/game_version     200  [{"version":"26.3-snapshot-8",…}]
GET  /api/v1/health                           200  {"status":"ok","version":"0.1.0"}
```

# 2 · Endpoints against `client.ts`

Both lists were produced by machine and **every** find was checked against the running service.
The multi-line `.route(…)` call was the trap: counting was done with a bracket counter, not with
`grep` per line. That gave 82 registrations with 106 method/path pairs; a line-by-line `grep`
would have missed `api/settings.rs:45-49` and `api/backups.rs:75`, among others.

All 106 pairs were called one by one. **105 answered from their handler**
(200/201/204/400/401/403/404-with-error-code/409). The one that fell through is K5.

The evidence, as agreed:

- `404` with `content-type: text/plain` and body `not found` → the route is missing. Happened
  once, on the deliberate control probe `/api/v1/definitely-not-a-route`.
- `404` with a JSON error code (`server_not_found`, `user_not_found`,
  `invitation_not_found`, `playit_claim_not_found`) → route there, thing not there.
- `400 invalid_request` with a field message → route and method there, my body was empty.
- `403`/`409` → route there, rule bites.

**Client → backend**: 93 pairs in `client.ts`, 92 have a route. The one without:
`GET /modrinth/v0/backups/{id}/download` (K5).

**Backend → client**: 14 pairs with no call from `client.ts`. Eight of them are playit and are
called from `web/src/api/playit.ts`, one is `/health`, one is the Modrinth forwarder over
`MODRINTH_PROXY_BASE`, one is `content/upload` over `uploadJson`. Two remain:
`/modrinth/v0/backups/{id}/download` (K5) and `PUT …/operations/{op}/payload` (T3).

**Client → interface**: 100 endpoint functions in `client.ts`, **six** of which are called by no
`.vue` file and no other `.ts` file:

| Function | Classification |
|---|---|
| `api.access.invitations` | **BROKEN** — K6 |
| `api.access.acceptInvitation` | **BROKEN** — K6 |
| `api.access.declineInvitation` | **BROKEN** — K6 |
| `api.content.installModpack` | **BROKEN** — K7 |
| `api.operations.putPayload` | dead — T3 |
| `api.backups.aliasDownloadUrl` | dead, because the route is missing — T4/K5 |

# 3 · Background tasks

Twelve places in production code contain an endless loop or a `tokio::spawn` that ought to run
exactly once at startup. **Six run, six do not.**

| Task | Start | Evidence |
|---|---|---|
| `hub.listen()` | `main.rs:104` | `INFO servers::hub: supervisor attached server=… pid=604673` |
| `playit.start()` | `main.rs:135` | `GET /api/v1/admin/playit` → `200 {"configured":false,"agent":{"state":"absent"}}` |
| `run_operations` | `main.rs:166` | `POST /servers` → the operation ran through in 5 s: `INFO servers::manager: a server was set up server=01KZWDG8W32A9FX2S6NQMXJ90A loader="paper" bytes=51437498` |
| `ops::follow` | `main.rs:175` | state change on the socket: `starting` → `running` in 39 ms |
| `audit::spawn_purge` | `main.rs:177` | ticks every 24 h, logs only when it has an effect — **not checkable inside one session**, which is a finding in itself (see below) |
| `sweep_upload_parts` | `main.rs:178` | ticks hourly, logs only when it has an effect — **not checkable** |
| `Operations::spawn_housekeeping` | — | **K4** |
| `Backups::spawn_scheduler` | — | **K3** |
| `Manager::spawn_metrics` | — | **K2** |
| `Manager::spawn_recovery` | — | **K1** |
| `Manager::spawn_dispatcher` | — | **K8** |
| `Content::sweep_updates` | `main.rs:175` | ticks every 15 min and takes only servers that are due (`content/mod.rs:793`); logs only when it has an effect — **not checkable inside one session**, evidenced by `content::tests` over `sweep_once` |

An addendum to this table: the five rows with "—" are out of date. `main.rs:155` and
`main.rs:176`–`179` start `spawn_recovery`, `spawn_metrics`, `spawn_dispatcher`,
`spawn_housekeeping` and `spawn_scheduler`; the sections on K1–K4, K8 and "The lines missing from
`main.rs`" describe a state that no longer exists. Only T1 has been brought up to date here,
because the update check hangs on it.

Two tasks are not checkable, because on success they keep quiet and their effect would only show
after hours: `audit::spawn_purge` (24 h) and `sweep_upload_parts` (1 h). Both demonstrably stand
in `main.rs`, but nobody has seen them tick. One `tracing::debug!` line per round would settle
that for good.

The evidence for T1, that the substitute path holds:

```
GET /api/v1/servers/01KZWDG8…/content   200  {"…","updates_checked_at":null,…}
   five seconds later in the database:
   updates_checked_at = '2026-08-13T02:11:55Z'
```

`list()` kicks off the check itself. The background sweep would only do it ahead of time.

# 4 · Provider fields

Eight contracts from `vendor/modrinth/ui/src/**/providers/*.ts`, **253 fields** in total.

| Contract | Fields | Required | Optional | Filled | Quietly empty |
|---|---:|---:|---:|---:|---:|
| `ModrinthServerContext` | 22 | 22 | 0 | 22 | 0 |
| `ConsoleManagerContext` | 18 | 1 | 17 | 18 | 0 |
| `FileManagerContext` | 40 | 21 | 19 | 32 | 8 |
| `ContentManagerContext` | 42 | 15 | 27 | 37 | 5 |
| `BrowseManagerContext` | 64 | 36 | 28 | 53 | 11 |
| `InstallationSettingsContext` | 50 | 29 | 21 | 48 | 2 |
| `ServerSettingsContext` | 5 | 4 | 1 | 4 | 1 |
| `UserProfileContext` | 12 | 12 | 0 | 0 | 12 |
| **Total** | **253** | **140** | **113** | **214** | **39** |

**No required field is empty.** Shown with `vue-tsc --noEmit`: 710 errors, **zero** of them in
`web/src`. With `const context: BrowseManagerContext = { … }` a missing required field would show
up at once. Careful: `pnpm build` does *not* check that (T12), only the separate `typecheck`
script does, and it runs nowhere by itself.

The 39 empty fields are all optional or belong to a layout that is never embedded. Every use
site in the vendor is guarded with `?.` or `v-if`; there is no empty area, only one button
fewer:

| Empty | Where it shows | Verdict |
|---|---|---|
| `canShareToMclogs`, `shareToMclogs` | `files-tab/components/FileNavbar.vue:199` — the "send to mclogs" button is missing from the file manager | dead. The backend can do it (`POST /console/crash-analysis`), just not from here |
| `openInFolder`, `canRestart`, `restartServer`, `showInstallFromUrl` | only the Modrinth app shows them | harmless |
| `downloadButtonLabel`, `uploadingLabel` | labels with a fallback | harmless |
| `getDeleteWarning`, `getDisableWarning` | `content-tab/layout.vue:368`, `:538`, both `?? null` | dead. The dependency warning is built over a path of its own (`content-manager.ts:680` calls `client.content.dependents`) |
| `bulkUpdateItem` | `layout.vue:661` checks `bulkUpdateAll ‖ bulkUpdateItem ‖ bulkUpdateItems` — the other two are filled | harmless |
| `getOverflowOptions`, `shareItems` | `layout.vue:1039` with `v-if` | harmless |
| 11 fields in `BrowseManagerContext` | labels, mouse events, `loadingComponent`, `serverPings` | harmless |
| `managedModpackWarning`, `afterSave` | `installation-settings/layout.vue:683` with `?.` | harmless as long as K7 holds |
| `closeModal` | modal variant | harmless |
| 12 fields `UserProfileContext` | `UserProfilePageLayout` is never embedded | harmless |

`BrowseManagerContext` is the place where a naive comparison throws 18 false alarms:
`browse-manager.ts:525` spreads the result of Modrinth's `useBrowseSearch` in with
`...searchState`. Search only for keys in the file and you report `totalHits`, `setPage`,
`currentPage` and fifteen more as missing. They are there.

# 5 · Public functions with no caller

728 public functions in production code. **43** are called nowhere outside their own file and
outside tests.

Sorted:

**Entry points somebody should have started — 7 findings**

| Function | File | Finding |
|---|---|---|
| `Manager::spawn_recovery` | `servers/manager.rs:1439` | K1 |
| `Manager::spawn_metrics` | `servers/manager.rs:1501` | K2 |
| `Backups::spawn_scheduler` | `backups/schedule.rs:206` | K3 |
| `Operations::spawn_housekeeping` | `ops/mod.rs:353` | K4 |
| `api::backups::compat_router` | `api/backups.rs:88` | K5 |
| `Manager::spawn_dispatcher` | `servers/manager.rs:616` | K8 |
| `Content::sweep_updates` | `content/mod.rs:774` | ~~T1~~ — called in `main.rs:175` |

**No reference at all, not even in tests — 4**

`auth::extract::session_ref`, `Backups::used_quota`, `Modrinth::project_is_fresh`,
`Channel::seconds_since`. Candidates for deletion.

**Dead indirectly, alive again with K1–K4 — 5**

`Manager::adopt_tokens`, `Operations::housekeeping`, `Operations::sweep_work_dirs`,
`Backups::prune`. `Content::sweep_once` (and with it `check_updates_gently`) hangs on
`sweep_updates` and has been running since `main.rs:175`.

**Helpers, harmless — 27**

Called only internally or only from tests. Listed under HARMLESS.

---

# The lines missing from `main.rs`

`main.rs` was not touched. The following six changes close K1 through K5 and K8. All types are
checked: `Operations::new`, `Backups::new`, `Content::new` and `Manager::new` all return an
`Arc<…>`, and all five `spawn_*` take `self: &Arc<Self>`.

**1 · Replace line 151** — do not add to it. `spawn_recovery` does `adopt_tokens`, waits out
`REATTACH_GRACE` and then calls `reconcile` itself. If the old line stayed, it would clear the
server away before the grace period begins.

```rust
-    manager.reconcile().await;
+    manager.spawn_recovery();
```

**2 · Insert after line 178** (with the other starters):

```rust
    manager.spawn_metrics();
    manager.spawn_dispatcher();
    operations.spawn_housekeeping();
    backups.spawn_scheduler();
    content.sweep_updates(live.clone());
```

`content.sweep_updates(…)` has to stand before line 186, because `content` is consumed there in
`api::content::router(content, live.clone())`. If you do not want to change the order, write
`Arc::clone(&content)` there.

`manager.spawn_dispatcher()` is the optional part: `run_operations` does the same work, only
serially (K8). With both at once there is no double start, because `operations.begin()` hands out
the claim atomically.

**3 · Line 189** — `backups` is needed twice over:

```rust
-        .merge(api::backups::router(backups))
+        .merge(api::backups::router(Arc::clone(&backups)))
```

**4 · Line 197** — `state` likewise:

```rust
-        .with_state(state);
+        .with_state(state.clone());
```

**5 · Line 200/201** — the alias path belongs at the root, not under `/api/v1`:

```rust
     let app = Router::new()
         .nest("/api/v1", api)
+        .merge(api::backups::compat_router(backups).with_state(state))
         .merge(web::router())
         .layer(TraceLayer::new_for_http());
```

It has to stand **before** `web::router()`; otherwise the SPA fallback route `/{*path}` swallows
it again.

**6 · Not in `main.rs`, but open**: K6 (invitation inbox) and K7 (modpacks) are gaps in the
interface. The backend and `client.ts` are done; what is missing is a route in
`web/src/router.ts` with a page for `/invitations`, and a branch in the create wizard as well as
a call to `client.content.installModpack` in the content tab.

---

# Test bench

| | |
|---|---|
| `cargo build -p craftpanel` | **green** — `Finished dev profile in 10.52s`, 8 warnings |
| `cargo test -p craftpanel` | **green** — `909 passed; 0 failed; 7 ignored`. (One intermediate run was red with two `E0063: missing field java_major` in `loaders/mod.rs:232` and `servers/manager.rs:2090` — another agent was in the middle of that extension and has finished it since. This run touched not one line of Rust.) |
| `pnpm --filter @craftpanel/web build` | **green** — `✓ built in 4.14s` |
| `pnpm --filter @craftpanel/web typecheck` | 710 errors, all in `vendor/` and `node_modules/`, **0 in `web/src`** |

Cleaned up: test server `wireaudit-probe` (512 MiB, Paper 1.21.4) created, started, stopped and
deleted: `GET /api/v1/servers` gives `{"servers":[],"users":{}}` again. The playit claim started
by accident while probing was taken back with `DELETE /api/v1/admin/playit/claim` → `204`;
`GET /api/v1/admin/playit` reports `configured: false` again.

One remnant is left: the test account **`wireaudit`** (`01KZWD77F5KW5GE6YN6V8DXH76`, no servers,
no session any more). It cannot delete itself: `api/admin.rs:426` forbids that with
`403 cannot_delete_self`, and `craftpanel admin` only knows `create`. Another administrator
clears it away with one call:

```
DELETE /api/v1/admin/users/01KZWD77F5KW5GE6YN6V8DXH76?servers=delete
```
