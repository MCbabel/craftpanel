# Creating, operations and progress

A cross-cutting area. Every long-running operation in the panel — create a server, install a
loader, install a modpack, install content in bulk, create a backup, restore a backup, unpack an
archive, delete a server — should use **one** model, **one** table, **one** WebSocket message and
**one** set of endpoints. The other areas only start operations.

Today this is a proposal, not a description: four sibling documents already carry operation models
of their own. What collides and who wins is in section 0. Without that decision the rest of this
document is just the fifth variant.

Source references are relative to `vendor/modrinth/ui/src/` and `vendor/modrinth/api-client/src/`.
**Exception:** `layouts/wrapped/**` and `components/servers/ServerListing.vue` are not there — they
were not vendored along. Line references to `root.vue`, `files.vue`, `content.vue`,
`onboarding.vue`, `index.vue`, `audit-log-utils.ts` and `ServerListing.vue` apply against the
reference clone `/root/ref-modrinth/packages/ui/src/` (commit `2a43792f`, the same state).
Everything under `wrapped/` is **not adoptable** per `docs/PLAN.md:76`; such references are a model
and bind nothing.

---

## 0. Relationship to the other documents

Four areas have already described their progress, each one differently. As long as that is not
resolved, there are five operation models instead of one.

| Place | there | here |
|---|---|---|
| `files.md:662,689,699` | own endpoints `…/files/operations[/{id}/cancel\|dismiss]` | `…/operations/:op_id/cancel\|dismiss` (2.4, 2.5) |
| `files.md:640-655,786-800` | WS `file_ops`, field `op: "extract"`, envelope `{ops:[…]}` | WS `operations`, field `kind: "unarchive"` (3.1) |
| `files.md:610,616` | states `failed-path`, `failed-corrupt` | `state: "failed"` plus `error.code` (4.1) |
| `files.md:696,707` | cancel `204`, finished operations expire after 10 minutes | cancel `200` with the operation, retention 7 days (2.4, 4.7) |
| `files.md:585,664,691` | permissions as `files:read`/`files:write` | bit names `BASE_READ`/`FILES_WRITE` (2.1, 2.4) |
| `content.md:1048-1070` | WS `content_task` with `task_id`, `stage`, `current`/`total` | WS `operations` with `phase`, `progress` |
| `settings.md:1054-1074` | WS `install_state` with `job_id`, `step` | likewise |
| `backups.md:669-675` | WS `backup_progress` with `backup_id`/`operation_id` | likewise |
| `servers.md:272-389` | `POST /api/v1/servers`: `source`/`mc_version`/`ram_mib`/`accept_eula`, `.mrpack` as `multipart`, `201` with the full `Server`, version resolved **before** the 201 | 2.7: `content`/`game_version`/`memory_mib`/`eula_accepted`, `.mrpack` through 2.8, `201` with `{server_id, operation}` |
| `servers.md:419-434` | `DELETE /api/v1/servers/:id`: `204`, record gone at once, open sockets closed with `4404` | 2.9: `202` with an operation, `status: "deleting"`, socket stays open |
| `servers.md:267-270` | the server list polls the **server list** every 5 s | 3.2: the server list also polls `GET /operations` every 5 s |

**Proposal for resolving this**, so that a fifth truth does not appear here:

1. This document governs the data model, the state names, the WS message and the generic
   endpoints. The area documents drop their own operation endpoints and messages and point here.
2. `POST /api/v1/servers` and `DELETE /api/v1/servers/:id` belong to the **server area**
   (`servers.md`). This document only describes what the operation part of them requires: a database
   row before the work (3.2) and a `server_create`/`server_delete` in the same table. The field
   names in 2.7 and 2.9 are **proposals to `servers.md`**; where they differ, `servers.md` wins.
3. Until that is decided, every difference above is an open issue, not a detail.

---

## 1. The provider contract

### 1.1 `ModrinthServerContext` — complete, field by field

Source of the interface: `providers/server-context.ts:37-72`. Four fields belong to this area, the
rest are listed here only with their owner, so that the list is complete.

| Field | Source | Owner | Where the value comes from |
|---|---|---|---|
| `serverId` | `server-context.ts:38` | server area | route parameter |
| `worldId` | `:39` | — | **the constant `"default"`**, see 5.1 |
| `server` | `:40` | server area | `GET /api/v1/servers/:id`. **But:** `server.status` is driven by this area, see 1.4 |
| `serverFull` | `:41` | server area | `GET /api/v1/servers/:id` (extended form) |
| `currentUserPermissions` | `:42` | accounts/roles | bitmask from the server object |
| `isConnected` | `:45` | console | WebSocket state |
| `isWsAuthIncorrect` | `:46` | console | WebSocket closes with 4401 |
| `powerState` | `:47` | console/metrics | WS message `state` |
| `powerStateDetails` | `:48` | console/metrics | WS message `state` |
| `isServerRunning` | `:49` | console/metrics | derived from `powerState` |
| `stats` | `:50` | metrics | WS message `stats` |
| `uptimeSeconds` | `:51` | metrics | WS message `state` |
| **`isSyncingContent`** | `:54` | **here** | `busy_reasons.includes("syncing_content")` |
| **`busyReasons`** | `:57` | **here** | `busy_reasons[]` from the WS message `operations`, see 1.3 |
| `fsAuth` | `:60` | — | **A gap, deliberately.** Modrinth talks to a second service (Kyros) and needs a token of its own for it. We have one process and one session cookie. The provider sets `ref(null)`; `refreshFsAuth` is an empty `async` function. No layout reads `fsAuth` — checked: the only readers of the contract field are `server-manage-core-runtime.ts:385,417`, and we replace that one (see 5.2). **Not** through the contract, but around it, two adopted building blocks fetch their token themselves: `composables/use-server-image.ts:67-78` and `components/servers/edit-server-icon/EditServerIcon.vue:153-234`. They still need a replacement; that belongs to the files area |
| `fsOps` | `:61` | **here** (raw form) | is **not** filled by our provider; we set `activeOperations` directly. `fsOps` stays `ref([])`. Outside the runtime the only thing that touches it is `root.vue:1106-1115`, and that is `wrapped/` — checked with `grep -rn "fsOps\|fsQueuedOps" vendor/modrinth/ui/src`: two hits, both the type declaration |
| `fsQueuedOps` | `:62` | **here** (raw form) | likewise, `ref([])` |
| `refreshFsAuth` | `:63` | — | a stub, see `fsAuth` |
| `uploadState` | `:66` | files | purely client-side from the XHR progress, see 2.8 |
| `cancelUpload` | `:67` | files | purely client-side, `xhr.abort()` |
| **`activeOperations`** | `:70` | **here** | projection of `operations[]`, see 1.2 |
| **`dismissOperation`** | `:71` | **here** | `POST …/operations/:op_id/dismiss` or `/cancel` |

### 1.2 `FileOperation` — the target form of `activeOperations`

Source: `layouts/shared/files-tab/types.ts:32-41`. Only operations of kind `unarchive` are projected
into it (reasoning in 4.3).

| Field | Declared | Actually used | Origin |
|---|---|---|---|
| `id?: string` | optional | **required.** Without `id` there is no cancel button (`FileOperationAdmonition.vue:26`) and no dismissing (`ServerPanelAdmonitions.vue:238`) | `Operation.id` |
| `op: string` | required | only as part of the stack key (`ServerPanelAdmonitions.vue:236`) | `Operation.kind`, that is `"unarchive"` |
| `src: string` | required | **really required.** `FileOperationAdmonition.vue:96` calls `props.op.src.includes(...)` without a check — `undefined` throws | `Operation.src`. Always set for `unarchive` |
| `state: string` | required | `=== 'done'` → green, `startsWith('fail')` → red, `=== 'queued'` → waiting (`FileOperationAdmonition.vue:3,7,94`; `ServerPanelAdmonitions.vue:182-193`) | `Operation.state` **verbatim**. That is why our error state is called `failed` and not `error`: `"failed".startsWith("fail")` is true |
| `progress?: number` | optional | **0…1**, not 0…100. `Admonition.vue:148` clamps to `[0,1]`, `FileOperationAdmonition.vue:5` passes it through | `Operation.progress` |
| `bytes_processed?: number` | optional | always shown, fallback value `0` (`FileOperationAdmonition.vue:17-20`) | `Operation.bytes_processed` |
| `files_processed?: number` | optional | read by no component — checked, only the type declaration names it | `Operation.files_processed`, carried along for our own display |
| `current_file?: string` | optional | shown when set (`FileOperationAdmonition.vue:22-24`) | `Operation.current_file` |

**What the contract does not take up: `cancellable`.** `FileOperation` has no such field, and
`FileOperationAdmonition.vue:26-37` shows the cancel button for **every** unfinished operation with
an `id`, without asking. So an `unarchive` past `applied_at` sits in the list with
`cancellable: false` and a clickable button in front of it all the same; the
`409 operation_not_cancellable` from 2.4 lands in `dismissOperation`, which only writes the error to
the console (`server-manage-core-runtime.ts:374-377`). The user sees nothing happen at all. Either
our own stack component (5.3) hides the button based on `cancellable`, or `unarchive` stays
cancellable to the end. **Open, belongs to the files area.**

**An observed defect in the vendored component:** `isTerminal`
(`FileOperationAdmonition.vue:94`) knows only `done` and `fail*`. An operation in state `cancelled`
counts as running there, so it keeps showing a cancel button and cannot be dismissed. **Our
provider filters `cancelled` out of `activeOperations`.** That makes the cancelled operation
disappear from the interface at once, which is the expected behavior anyway.

### 1.3 `BusyReason` — and why the ids are not free to choose

Source: `providers/server-context.ts:9-11`. `BusyReason.reason` is a `MessageDescriptor`, not text.
So the value **cannot** come from the server; the provider builds it from a machine-readable code.

Four message ids are **compared as strings** in the vendored code. Anyone who names them
differently breaks the deduplication between banner and warning. (`use-server-power-action.ts` sits
under `components/servers/server-header/`, not in `composables/`.)

| our code | message id the provider must produce | where it is compared |
|---|---|---|
| `installing` | `servers.busy.installing` | `ServerPanelAdmonitions.vue:67,76`; `use-server-power-action.ts:27`; `content.vue:183,188` |
| `syncing_content` | `servers.busy.syncing-content` | `ServerPanelAdmonitions.vue:67,76`; `use-server-power-action.ts:28`; `content.vue:183,188` |
| `backup_creating` | `servers.busy.backup-creating` | `ServerPanelAdmonitions.vue:72`; `files.vue:64`; `content.vue:192` |
| `backup_restoring` | `servers.busy.backup-restoring` | `ServerPanelAdmonitions.vue:72`; `files.vue:65`; `content.vue:193` |
| `deleting` | `servers.busy.deleting` — **new, ours** | compared nowhere; ends up in the general branch |

An unknown code is harmless: it falls into `filteredBusyReasons`
(`ServerPanelAdmonitions.vue:79-85`) and appears as a general warning "Background task running". A
*wrongly named* code is dangerous: then the interface shows banner and warning at the same time.

The effect of `busyReasons.length > 0`, measured, not assumed. The first two rows are adopted code
and therefore binding, the last row is our own page:

| Effect | Where | binding? |
|---|---|---|
| All power actions locked, kill included | `components/servers/server-header/use-server-power-action.ts:39-44,52-60` | **yes**, `components/servers/` |
| Settings "General", "Properties", "Installation" count as updating | `general.vue:140`, `properties.vue:282`, `installation.vue:203-216` | **yes**, `layouts/shared/` |
| File manager and content page locked | `files.vue:53-57`, `content.vue:174` | no — `wrapped/`, we write those pages ourselves |

The third row is not free either: the adopted file manager hangs its 25 locks on the **optional**
contract field `isBusy` (`layouts/shared/files-tab/layout.vue:307` and 24 further places), and the
content page does the same (`layouts/shared/content-tab/layout.vue:283-286,419,483`). What fills
`isBusy` is our decision; that a set `isBusy` locks everything is the library's decision.

From that it follows: **every** operation with a `busy_reason` locks start and stop. At this point
that is a decision of the vendored interface, not ours; our server has to enforce the same locks,
otherwise the 409 answer diverges from the grayed-out button.

### 1.4 `SyncProgress` and `ContentError` — the props of `InstallingBanner`

Source: `components/servers/InstallingBanner.vue:55-63`. The banner is a pure props component, it
injects nothing. It is handed in through `ServerPanelAdmonitions` (`:403-413`), which passes the
props through unchanged (`root.vue:362-364`).

| Field | Source | Origin |
|---|---|---|
| `SyncProgress.phase` | `:56` | `Operation.phase`, mapped through the table in 4.2 |
| `SyncProgress.percent` | `:57` | `Operation.progress * 100` — the banner divides by 100 again itself (`:201`) |
| `ContentError.step` | `:61` | `Operation.error.step` |
| `ContentError.description` | `:62` | `Operation.error.message` |

Two quirks that shape the contract:

1. **The banner is not shown during `Analyzing`** (`ServerPanelAdmonitions.vue:179`). Our phase
   `analyzing` is therefore meant for work under one second only — plain resolution of version and
   download address, no network traffic with progress.
2. **The banner translates certain error texts verbatim** (`InstallingBanner.vue:150-176`):
   `step === "modloader"` with `description` equal to `"the specified version may be incorrect"`,
   `"this version is not yet supported"` or `"internal error"`; `step === "modpack"` with a
   `description` that contains `"no primary file"` or `"failed to install"`. If nothing matches,
   `description` is shown raw (`:175`). **Recommendation:** our backend sends exactly these strings
   where they fit — then we get the translated messages for free. The mapping is in 4.5.

`server.status === 'installing'` is **one of three equal** conditions for the banner, joined with
OR: the status, `isSyncingContent` or a matching `busy_reason`
(`ServerPanelAdmonitions.vue:61-69`). So the banner would come from `busy_reasons` alone. We set the
status anyway, because `use-server-power-action.ts:23`, `installation.vue:205` and `ServerListing`
read it separately. Modrinth does the same from the WS state (`root.vue:713-717`, there from
`state.progress != null`):

```ts
watch(busyReasons, () => {
  if (busyCodes.value.includes('installing')) server.value.status = 'installing'
})
```

### 1.5 `FileManagerContext` — the part that belongs to this area

Source: `layouts/shared/files-tab/providers/file-manager.ts`.

| Field | Source | Origin |
|---|---|---|
| `activeOperations?` | `:50` | the same projection as 1.2, passed through from the server context |
| `dismissOperation?` | `:51` | the same function as 1.2 |
| `isBusy?` | `:41` | `busyReasons.length > 0`. Model `files.vue:53` (`wrapped/`, so our decision); the effect sits in `files-tab/layout.vue:307` |
| `busyTooltip?` | `:42` | text of the first `busyReason`, model `files.vue:54-56` |
| `busyWarning?` | `:43` | text of the first non-backup `busyReason`, model `files.vue:61-73` |
| `extractFile?` | `:45-49` | files area. The dry run (`dry: true`) answers **synchronously** with `ExtractDryRunResult`; only the real run creates an operation. Source: `layouts/shared/files-tab/layout.vue:504-543` |

### 1.6 `BackupAdmonitionEntry` — mapping, not ownership

Source: `components/servers/admonitions/BackupAdmonition.vue:18-30`. The backups area owns this
display, but it feeds on the same operations. So that the two areas do not carry two truths, here is
the fixed mapping:

| Field | Origin |
|---|---|
| `key` | `Operation.id` |
| `backupId` | `Operation.target_id` |
| `type` | `backup_create` → `'create'`, `backup_restore` → `'restore'` |
| `state` | `queued` → `'pending'`, `ongoing` → `'ongoing'`, `done` → `'completed'`, `failed` → `'failed'`, `cancelled` → `'cancelled'`. We never produce the state `timed_out` |
| `progress` | `Operation.progress` (0…1) |
| `operationId` | `Operation.id` — **but the type does not fit**, see below |
| `syntheticLegacy` | always `false` |
| `name` | name of the backup, from the backups area |
| `timestamp` | `Operation.finished_at ?? Operation.started_at ?? Operation.created_at` |
| `error` | `Operation.error?.message ?? null` |

**The one place where the vendored type does not suit us.** `operationId` is `number | null`
(`BackupAdmonition.vue:24`), our operation id is a ULID string (`backups.md:675` sends it as a
ULID as well). `BackupAdmonition.vue` is one of the four displays we want to adopt **unchanged** per
5.3 — so the contradiction shows up at the first `tsc`. The component does no arithmetic with the
number, it only checks for `null` (`ServerPanelAdmonitions.vue:289,320`;
`BackupAdmonition.vue:89-95`). Three ways, to be decided in the backups area:

1. Widen the type in the vendor copy to `string | number | null`: one line, but the first change to
   adopted code.
2. Fill `operationId` with a sequence number of our own per backup operation and keep the ULID only
   in `key` — then the component stays untouched, but we drag a second identifier along.
3. Rebuild `BackupAdmonition` as well — then only half of the four displays are still foreign.

---

## 2. The endpoints

Nine of them. All under `/api/v1/`, all with a session cookie, all errors in the form
`{ "error": "<code>", "message": "<text>" }`.

Permissions name the bits from `composables/server-permissions.ts:15-32`; which role has which bit
is decided by the accounts area.

### 2.1 `GET /api/v1/servers/:id/operations`

The operations of one server. The place to go after a page reload and for everything that has no
socket open.

Query parameters: `state` (`active` — the default — or `all`), `include_dismissed`
(`false` — the default), `limit` (default 50, maximum 200), `before` (ULID; returns only older
operations).

**Sorting and pagination.** Sorting is always descending by `id`: the ULID is time-sorted, no
second sort column is needed. `limit` on its own would be silent truncation: with `state=all` and a
retention of seven days (4.7) more than 200 rows pile up easily. To page further, send the smallest
`id` you have seen as `before`. This area needs no more paging machinery than that — the normal case
is `state=active` with a handful of rows.

Permission: `BASE_READ`.

Response `200`:

```json
{
  "operations": [
    {
      "id": "01JZ8QK3F0V6WQ0X6M2N9CQ7RT",
      "server_id": "01JZ8QJ9T4YB1S8HK4P0ZDA3WM",
      "kind": "unarchive",
      "state": "ongoing",
      "phase": null,
      "progress": 0.42,
      "message": "Extracting archive",
      "src": "/plugins/pack.zip",
      "bytes_processed": 18874368,
      "files_processed": 91,
      "current_file": "plugins/EssentialsX/config.yml",
      "error": null,
      "cancellable": true,
      "target_id": null,
      "started_by": "01JZ8Q9V0RS2H5PT7YF3D1XKAE",
      "created_at": "2026-08-12T14:03:11Z",
      "started_at": "2026-08-12T14:03:11Z",
      "finished_at": null,
      "dismissed_at": null
    }
  ],
  "busy_reasons": []
}
```

Errors: `401 unauthenticated`, `403 forbidden`, `404 server_not_found`.

### 2.2 `GET /api/v1/operations`

The same list across all servers the caller may see. That way the server list gets by without a
second WebSocket (see 3.2). An administrator sees all servers, an ordinary user sees their own and
the ones they hold a server role on.

Query parameters: `state` (`active` — the default — or `all`), `server_id` (allowed more than once),
`limit` (default 100, maximum 200), `before` (ULID). Sorting and pagination as in 2.1.
`busy_reasons_by_server` is unaffected by all of that and always lists every visible server with a
lock — the number is bounded by the machine's server count.

Permission: signed in; filtered per server through `BASE_READ`.

Response `200`:

```json
{
  "operations": [
    {
      "id": "01JZ8QM7A2E4N7T1V5C8H3RGPD",
      "server_id": "01JZ8QJ9T4YB1S8HK4P0ZDA3WM",
      "kind": "server_create",
      "state": "ongoing",
      "phase": "installing_loader",
      "progress": 0.31,
      "message": "Downloading paper-1.21.4-151.jar",
      "src": null,
      "bytes_processed": 14680064,
      "files_processed": null,
      "current_file": null,
      "error": null,
      "cancellable": true,
      "target_id": null,
      "started_by": "01JZ8Q9V0RS2H5PT7YF3D1XKAE",
      "created_at": "2026-08-12T14:00:02Z",
      "started_at": "2026-08-12T14:00:02Z",
      "finished_at": null,
      "dismissed_at": null
    }
  ],
  "busy_reasons_by_server": {
    "01JZ8QJ9T4YB1S8HK4P0ZDA3WM": ["installing"]
  }
}
```

Errors: `401 unauthenticated`.

### 2.3 `GET /api/v1/servers/:id/operations/:op_id`

A single operation. Needed when a caller wants to wait for the end on purpose after a
`202 Accepted`, without opening the socket (scripts, tests).

Permission: `BASE_READ`.

Response `200`: the `Operation` object as in 2.1, without an envelope.

```json
{
  "id": "01JZ8QM7A2E4N7T1V5C8H3RGPD",
  "server_id": "01JZ8QJ9T4YB1S8HK4P0ZDA3WM",
  "kind": "server_create",
  "state": "done",
  "phase": null,
  "progress": 1,
  "message": "Server ready",
  "src": null,
  "bytes_processed": 47185920,
  "files_processed": null,
  "current_file": null,
  "error": null,
  "cancellable": false,
  "target_id": null,
  "started_by": "01JZ8Q9V0RS2H5PT7YF3D1XKAE",
  "created_at": "2026-08-12T14:00:02Z",
  "started_at": "2026-08-12T14:00:02Z",
  "finished_at": "2026-08-12T14:01:44Z",
  "dismissed_at": "2026-08-12T14:01:44Z"
}
```

Errors: `401 unauthenticated`, `403 forbidden`, `404 operation_not_found`.

### 2.4 `POST /api/v1/servers/:id/operations/:op_id/cancel`

Cancels a running or waiting operation. No request body.

Permission: by kind of operation — `unarchive` → `FILES_WRITE`
(`FileOperationAdmonition.vue:110` checks exactly that), `backup_create`/`backup_restore` →
`BACKUPS` (`ServerPanelAdmonitions.vue:316`), `server_create` → `SETUP`, `server_delete` → the
owner or a panel administrator.

Response `200`: the operation in state `cancelled`, or still `ongoing` if the cancel has been
requested but not carried out yet. The caller should rely on the WS message, not on this response.

```json
{
  "id": "01JZ8QK3F0V6WQ0X6M2N9CQ7RT",
  "server_id": "01JZ8QJ9T4YB1S8HK4P0ZDA3WM",
  "kind": "unarchive",
  "state": "cancelled",
  "phase": null,
  "progress": 0.42,
  "message": "Cancelled",
  "src": "/plugins/pack.zip",
  "bytes_processed": 18874368,
  "files_processed": 91,
  "current_file": null,
  "error": null,
  "cancellable": false,
  "target_id": null,
  "started_by": "01JZ8Q9V0RS2H5PT7YF3D1XKAE",
  "created_at": "2026-08-12T14:03:11Z",
  "started_at": "2026-08-12T14:03:11Z",
  "finished_at": "2026-08-12T14:03:58Z",
  "dismissed_at": "2026-08-12T14:03:58Z"
}
```

Errors: `401 unauthenticated`, `403 forbidden`, `404 operation_not_found`,
`409 operation_not_cancellable` (a kind of operation without cancel, or `cancellable: false` because
the point of no return has been passed), `409 operation_already_finished`.

### 2.5 `POST /api/v1/servers/:id/operations/:op_id/dismiss`

Dismisses a **finished** operation. Sets `dismissed_at`; after that it appears neither in the
snapshot nor in `GET …/operations` (except with `include_dismissed=true`). No request body.

On the server, not only in the browser — Modrinth does both
(`server-manage-core-runtime.ts:368-378` sets a local set *and* calls the server). We only do the
server side: then what was dismissed stays dismissed, even after a page reload.

Permission: `BASE_READ`. Deliberately lower than cancel: the vendored component checks no
permissions when dismissing (`ServerPanelAdmonitions.vue:382-386` calls without a check), but it
does when cancelling (`FileOperationAdmonition.vue:110`).

Response `204`, no body.

Errors: `401 unauthenticated`, `403 forbidden`, `404 operation_not_found`,
`409 operation_still_running`.

### 2.6 `POST /api/v1/servers/:id/operations/:op_id/retry`

Repeats a failed operation with **the same** inputs. Creates a **new** operation and dismisses the
old one. Modrinth wires the banner's retry button to `repair` (`root.vue:1083-1099`), so to a fresh
operation as well, not to a continuation.

No request body. Not possible for `unarchive` (the source file may be gone by now) and for
`server_delete`.

Permission: `SETUP` for all `install_*`, `repair_content` and `server_create`
(`ServerPanelAdmonitions.vue:409` gates the button through `canSetup`), `BACKUPS` for `backup_*`.

Response `202`:

```json
{
  "operation": {
    "id": "01JZ8QR5C9K3M6P8S1W4Y7B2ZE",
    "server_id": "01JZ8QJ9T4YB1S8HK4P0ZDA3WM",
    "kind": "install_loader",
    "state": "queued",
    "phase": "analyzing",
    "progress": 0,
    "message": "Retry queued",
    "src": null,
    "bytes_processed": null,
    "files_processed": null,
    "current_file": null,
    "error": null,
    "cancellable": true,
    "target_id": null,
    "started_by": "01JZ8Q9V0RS2H5PT7YF3D1XKAE",
    "created_at": "2026-08-12T14:12:00Z",
    "started_at": null,
    "finished_at": null,
    "dismissed_at": null
  }
}
```

Errors: `401 unauthenticated`, `403 forbidden`, `404 operation_not_found`,
`409 operation_not_retryable`, `409 server_busy`.

### 2.7 `POST /api/v1/servers`

**The endpoint belongs to `servers.md:272-389`.** What is written here is this area's requirement on
it plus a proposal for the fields; where the two differ, `servers.md` wins (section 0).

Creates a server. The response comes **before** anything is downloaded. That is the core of the
answer to the socket question (see 3.1).

Inside the request only what works without the network happens: check the budget, take the port,
assign the ULID, write the database row, create the directory, write the operation row. Everything
else is done by the operation. The **system user** is not created here: it hangs off the panel user
and is created when the account is created (`docs/PLAN.md:138-141`, helper command `create-user`,
`:187`). A `useradd` through the root helper would not be a step to put into a request that is
supposed to answer quickly either.

**Budget and port have to be assigned in one transaction.** Otherwise two concurrent requests from
the same user read the same allocated total and both are allowed, or they grab the same free port.
So the check and the insert run in one SQLite transaction, and the port column carries a `UNIQUE`
constraint; a violation of it becomes `409 port_in_use`. Without that, `budget_exceeded` is a
recommendation, not a limit.

Permission: signed in. Only a panel administrator may set `owner_id`, and only a panel
administrator may set `port` (`docs/PLAN.md:350-354`).

Request, case "loader" (`content.kind: "loader"`):

```json
{
  "name": "Friends world",
  "owner_id": null,
  "memory_mib": 4096,
  "port": null,
  "eula_accepted": true,
  "content": {
    "kind": "loader",
    "loader": "paper",
    "game_version": "1.21.4",
    "loader_version": null
  },
  "properties": {
    "known": {
      "gamemode": "survival",
      "hardcore": "false",
      "difficulty": "normal",
      "level_seed": null,
      "level_type": "minecraft:normal",
      "generate_structures": "true"
    },
    "custom": {}
  }
}
```

`properties` is literally what the wizard delivers
(`components/flows/creation-flow-modal/creation-flow-context.ts:524-541`, type
`Archon.Content.v1.PropertiesFields`, `api-client/src/modules/archon/types.ts:419-422`).
`loader_version: null` means "latest stable build". The name comes from `worldName`
(`creation-flow-context.ts:140`). In the wizard that is the only name field for non-instance
flows, and `onboarding.vue` does not use it, because there the server name was already fixed at
purchase.

Request, case "modpack from the Modrinth search":

```json
{
  "name": "Create Above and Beyond",
  "owner_id": null,
  "memory_mib": 8192,
  "port": null,
  "eula_accepted": true,
  "content": {
    "kind": "modpack_project",
    "project_id": "sTZr7NVo",
    "version_id": "6ObGmpvz"
  },
  "properties": {
    "known": {
      "gamemode": "survival",
      "hardcore": "false",
      "difficulty": "normal",
      "level_seed": null,
      "level_type": "minecraft:normal",
      "generate_structures": "true"
    },
    "custom": {}
  }
}
```

Request, case "uploaded `.mrpack` file":

```json
{
  "name": "Custom pack",
  "owner_id": null,
  "memory_mib": 6144,
  "port": null,
  "eula_accepted": true,
  "content": {
    "kind": "modpack_upload",
    "file_name": "mypack.mrpack",
    "file_size": 184549376
  },
  "properties": {
    "known": {
      "gamemode": "survival",
      "hardcore": "false",
      "difficulty": "normal",
      "level_seed": null,
      "level_type": "minecraft:normal",
      "generate_structures": "true"
    },
    "custom": {}
  }
}
```

Response `201`, header `Location: /api/v1/servers/01JZ8QJ9T4YB1S8HK4P0ZDA3WM`:

```json
{
  "server_id": "01JZ8QJ9T4YB1S8HK4P0ZDA3WM",
  "operation": {
    "id": "01JZ8QM7A2E4N7T1V5C8H3RGPD",
    "server_id": "01JZ8QJ9T4YB1S8HK4P0ZDA3WM",
    "kind": "server_create",
    "state": "queued",
    "phase": "analyzing",
    "progress": 0,
    "message": "Preparing",
    "src": null,
    "bytes_processed": null,
    "files_processed": null,
    "current_file": null,
    "error": null,
    "cancellable": true,
    "target_id": null,
    "started_by": "01JZ8Q9V0RS2H5PT7YF3D1XKAE",
    "created_at": "2026-08-12T14:00:02Z",
    "started_at": null,
    "finished_at": null,
    "dismissed_at": null
  }
}
```

The response does **not** contain the whole server object. Reason: its form belongs to the server
area, and the detail page loads it right away anyway. A second version of the same schema in this
document would be the first place where the two drift apart.

Errors:

| Status | Code | When |
|---|---|---|
| `400` | `invalid_request` | field missing, name empty, `memory_mib` below 512 |
| `401` | `unauthenticated` | no session |
| `403` | `forbidden` | `owner_id` or `port` set by a non-administrator |
| `409` | `port_in_use` | the given port is taken |
| `409` | `no_free_port` | the administrator's pool is exhausted |
| `422` | `budget_exceeded` | sum of the allocated `-Xmx` plus the new one > the user's limit (`docs/PLAN.md:314-322`). **Exception:** a panel administrator may go up to the machine, and then a warning appears instead of the error (`docs/PLAN.md:354`) |
| `422` | `user_over_limit` | the user is already over, because the administrator lowered the limit (`docs/PLAN.md:364-366`) |
| `422` | `eula_not_accepted` | `eula_accepted` is `false` (`docs/PLAN.md:334-335`) |
| `422` | `unknown_loader` | loader not in our list (4.3: ten, `docs/PLAN.md:377-400`) |
| `422` | `unsupported_version` | the loader does not know this game version |
| `502` | `upstream_unavailable` | the loader source does not answer while resolving the version |

`upstream_unavailable` is listed here because resolving the version happens **before** the response
when it comes from the cache. If the cache is cold, it moves into the operation and becomes
`error.code = "upstream_unavailable"` there.

### 2.8 `PUT /api/v1/servers/:id/operations/:op_id/payload`

Delivers the payload of a waiting operation after the fact. Exactly one use case: the uploaded
`.mrpack` from 2.7. The operation stays `queued` until the body has arrived in full.

`Content-Type: application/octet-stream`, the body is the raw file. No `multipart` — there is
exactly one field, and the progress bar hangs off the XHR, not off the format.

Permission: `SETUP`.

Response `202` in the envelope from 2.10, the same one as for every other 202 in this area. The
operation is now in `queued` with `bytes_processed` filled, or already in `ongoing`.

```json
{
  "operation": {
    "id": "01JZ8QM7A2E4N7T1V5C8H3RGPD",
    "server_id": "01JZ8QJ9T4YB1S8HK4P0ZDA3WM",
    "kind": "server_create",
    "state": "ongoing",
    "phase": "analyzing",
    "progress": 0.02,
    "message": "Checking the pack",
    "src": "mypack.mrpack",
    "bytes_processed": 184549376,
    "files_processed": null,
    "current_file": null,
    "error": null,
    "cancellable": true,
    "target_id": null,
    "started_by": "01JZ8Q9V0RS2H5PT7YF3D1XKAE",
    "created_at": "2026-08-12T14:00:02Z",
    "started_at": "2026-08-12T14:02:39Z",
    "finished_at": null,
    "dismissed_at": null
  }
}
```

Why the operation itself is the target of the upload and not a staging area of its own: one
identifier less, and the cleanup rule from 4.7 applies without an addition: the half-finished
upload sits in the operation's work directory and dies with it. A waiting operation that has not
received a payload after **15 minutes** goes to `failed` with `error.code = "payload_timeout"`.

The upload progress does **not** come over the WebSocket. It comes from the `progress` events of the
XHR and fills `ctx.uploadState` (`providers/server-context.ts:66`, shown by
`UploadAdmonition.vue:60-66`). About a running upload the server knows nothing more precise than
"the body is not over yet".

Errors: `401 unauthenticated`, `403 forbidden`, `404 operation_not_found`,
`409 payload_not_expected` (the operation is not waiting for anything),
`409 payload_already_delivered`, `413 payload_too_large` (over the administrator's limit),
`422 invalid_modpack` (not a readable `.mrpack`).

### 2.9 `DELETE /api/v1/servers/:id`

Deletes a server. It is listed here because deleting a large world directory can take minutes and is
therefore an operation — not because this area owns server management. **`servers.md:419-434`
describes the same endpoint the opposite way** (`204`, record gone at once, open sockets closed with
`4404`). Both at once is impossible; the decision is in section 0 and has to be made in the server
area.

Permission: the server's owner or a panel administrator.

Response `202`:

```json
{
  "operation": {
    "id": "01JZ8QT1B4D7G0J3M6Q9T2X5AZ",
    "server_id": "01JZ8QJ9T4YB1S8HK4P0ZDA3WM",
    "kind": "server_delete",
    "state": "ongoing",
    "phase": null,
    "progress": 0,
    "message": "Deleting the server",
    "src": null,
    "bytes_processed": null,
    "files_processed": null,
    "current_file": null,
    "error": null,
    "cancellable": false,
    "target_id": null,
    "started_by": "01JZ8Q9V0RS2H5PT7YF3D1XKAE",
    "created_at": "2026-08-12T15:20:00Z",
    "started_at": "2026-08-12T15:20:00Z",
    "finished_at": null,
    "dismissed_at": null
  }
}
```

The server disappears **at once** from `GET /api/v1/servers` and is set to `status: "deleting"`; the
WebSocket stays reachable until the operation ends, so that an open detail page notices the end and
can navigate away.

Errors: `401 unauthenticated`, `403 forbidden`, `404 server_not_found`,
`409 server_running` (stop it first), `409 server_busy`.

### 2.10 The rule for all other areas

Every endpoint of another area that triggers a long-running operation answers **`202 Accepted`**
with exactly this envelope:

```json
{
  "operation": {
    "id": "01JZ8QV6E8H1L4N7R0U3W6Z9BC",
    "server_id": "01JZ8QJ9T4YB1S8HK4P0ZDA3WM",
    "kind": "install_content",
    "state": "queued",
    "phase": "addons",
    "progress": 0,
    "message": "3 items queued",
    "src": null,
    "bytes_processed": null,
    "files_processed": null,
    "current_file": null,
    "error": null,
    "cancellable": false,
    "target_id": null,
    "started_by": "01JZ8Q9V0RS2H5PT7YF3D1XKAE",
    "created_at": "2026-08-12T14:30:00Z",
    "started_at": null,
    "finished_at": null,
    "dismissed_at": null
  }
}
```

And every writing endpoint that runs into a lock answers `409`:

```json
{
  "error": "server_busy",
  "message": "This server is busy right now: installation"
}
```

The error envelope has exactly the two fields laid down. Which lock applies is only in the text —
whoever needs it machine-readable takes it from `GET …/operations`.

---

## 3. WebSocket messages

One socket per server: `/api/v1/servers/:id/ws`. This area contributes **one** kind of message.

### 3.1 `operations` — a full snapshot

```json
{
  "type": "operations",
  "busy_reasons": ["installing"],
  "operations": [
    {
      "id": "01JZ8QM7A2E4N7T1V5C8H3RGPD",
      "server_id": "01JZ8QJ9T4YB1S8HK4P0ZDA3WM",
      "kind": "server_create",
      "state": "ongoing",
      "phase": "installing_loader",
      "progress": 0.31,
      "message": "Downloading paper-1.21.4-151.jar",
      "src": null,
      "bytes_processed": 14680064,
      "files_processed": null,
      "current_file": null,
      "error": null,
      "cancellable": true,
      "target_id": null,
      "started_by": "01JZ8Q9V0RS2H5PT7YF3D1XKAE",
      "created_at": "2026-08-12T14:00:02Z",
      "started_at": "2026-08-12T14:00:02Z",
      "finished_at": null,
      "dismissed_at": null
    }
  ]
}
```

Rules:

- **Always the full state**, never a difference. Modrinth does it the same way
  (`WSFilesystemOpsEvent.all`, `api-client/src/modules/archon/types.ts:1143-1146`; the comparison is
  done with `JSON.stringify` in `root.vue:1104-1110`). A snapshot cannot arrive in the wrong order
  and needs no recovery after a dropped connection.
- It contains all operations of this server that were not dismissed — running **and** finished. That
  is why even a browser that connects only after the failure sees the red banner.
- Sent **right after the connection is established** and after that on every change of state.
- Pure progress changes are throttled to **one message per second**. State changes
  (`queued` → `ongoing` → `done`/`failed`/`cancelled`) and phase changes go out at once.
- `busy_reasons` rides along in the same message, so that the reason for the lock and the operation
  list can never drift apart. The server decides about the lock, not the browser. It is the one
  that enforces it with `409`, after all.

There is no message from the browser to the server in this area. Cancel, dismiss and retry go over
HTTP; Modrinth does the same (`server-manage-core-runtime.ts:373` calls an HTTP method, not a socket
message).

### 3.2 The trickiest question: a server without a socket

**The case:** "create a server" takes one to two minutes, but the socket hangs off a server id that
does not exist yet when "Create" is clicked.

**The answer in one sentence: the database row is created before the work, not after it.**

`POST /api/v1/servers` (2.7) does synchronously only what works without the network, and then
answers with `201` and the finished ULID. From that moment on

- `/api/v1/servers/:id/ws` is reachable,
- the server is in `GET /api/v1/servers` with `status: "installing"`,
- the operation exists in the database.

So the browser navigates to `/servers/:id`, opens the socket as for any other server and gets the
snapshot from 3.1 first. There is no window in which an operation is running but no socket would be
possible.

Three gaps remain, and all three are closed without introducing a second socket:

1. **Between the `201` and the connection** a few hundred milliseconds pass. Because the first
   message is a full snapshot and finished operations are included until they are dismissed, nothing
   is lost — even an operation that fails inside that gap is delivered afterwards.
2. **The server list** has no socket and is not to get one. It fetches
   `GET /api/v1/operations?state=active` every **five seconds** as long as at least one operation is
   running, and not at all once none is running any more. This is exactly the pattern Modrinth uses
   for the backup queue — there with 30 s, though, not 5 s (`composables/server-backups-queue.ts:24-27`:
   `refetchInterval` returns `30_000` as long as operations are running, otherwise `false`). What is
   adopted is the pattern, not the number; `servers.md:267-270` already names 5 s for the list, and
   the two requests should be merged.
   For the list that is entirely enough: while a server is being created, `ServerListing` shows only
   a spinner and a hint, no percentage (`ServerListing.vue:30-33,109-110`).
3. **The browser is closed and opened again later.** The operation keeps running in the backend; on
   coming back, either the list (case 2) or the socket (case 1) delivers the state. No state lives
   in the browser.

**Explicitly rejected:** a second, panel-wide WebSocket. It breaks the rule "one socket per server",
doubles the delivery path and brings no gain beyond immediate progress in the list — for a spinner a
second delivery path is not worth it.

**What this solution assumes, and is therefore mandatory:** the WebSocket endpoint must **not**
refuse a connection just because the server was never started, has no directory with content in it
or carries `status: "installing"`. It hangs off the database row, not off a process. That is a
requirement on the console area, and it is written here because otherwise nobody would notice it.

**The one assumption that really carries this:** every operation belongs to exactly one server.
There is no operation without a server in this panel, and that is why one socket per server is enough.
Should something panel-wide come along later (the panel updating itself, say), this model no longer
carries. That is a deliberately open gap, see 5.4.

---

## 4. Data types and operation rules

4.1 to 4.5 are the types — they will be adopted verbatim later. 4.6 to 4.8 are the three rules
without which the types mean nothing: who locks what, what happens on a restart, and how the
creation flow runs through both.

### 4.1 Core

```ts
export type OperationKind =
	| 'server_create'
	| 'server_delete'
	| 'install_loader'
	| 'install_modpack'
	| 'install_content'
	| 'repair_content'
	| 'backup_create'
	| 'backup_restore'
	| 'unarchive'
	// The other areas need more, see below. Reserved, meaning defined there:
	| 'update_content'
	| 'update_modpack'
	| 'change_game_version'
	| 'reinstall'
	| 'install_java'

export type OperationState = 'queued' | 'ongoing' | 'done' | 'failed' | 'cancelled'

export type OperationPhase = 'analyzing' | 'installing_loader' | 'installing_pack' | 'addons'

export type OperationErrorStep = 'modloader' | 'modpack' | 'download' | 'filesystem' | 'internal'

export interface OperationError {
	/** Stable and machine-readable. List in 4.5. */
	code: string
	/** For humans. Becomes ContentError.description. */
	message: string
	/** Becomes ContentError.step. */
	step: OperationErrorStep
}

export interface Operation {
	id: string
	server_id: string
	kind: OperationKind
	state: OperationState
	/** Set only for server_create, install_*, repair_content. */
	phase: OperationPhase | null
	/** 0…1, never 0…100. */
	progress: number
	/** Free text for our own interface. No vendored component reads it. */
	message: string | null
	/** Required for unarchive (the archive path), otherwise null. */
	src: string | null
	bytes_processed: number | null
	files_processed: number | null
	current_file: string | null
	error: OperationError | null
	/** May change to false during the run, when the point of no return falls. */
	cancellable: boolean
	/** backup_id for backup_create and backup_restore, otherwise null. */
	target_id: string | null
	/** User id; null when the panel started the operation itself. */
	started_by: string | null
	created_at: string
	started_at: string | null
	finished_at: string | null
	dismissed_at: string | null
}

export type BusyReasonCode =
	| 'installing'
	| 'syncing_content'
	| 'backup_creating'
	| 'backup_restoring'
	| 'deleting'
```

**The nine original kinds do not cover the panel.** The enum was missing the long-running operations
of the other areas: update content and update modpack (`content.md:1064`: `content_update`,
`modpack_update`), change the game version (`game_version_change`, same place), reinstall and reset
(`settings.md:1054`: `install`, `repair`, `reset`) and downloading a Java runtime (same place).
All of them are in the plan (`docs/PLAN.md:87-91`). They are taken into the list above so that the
claim of "one model" holds; their phases, busy reasons and permissions belong in the respective area
documents. `OperationPhase` is not enough for them: `install_java` and `change_game_version` need
values of their own, and only whoever displays them may invent those.

### 4.2 Messages and response envelopes

```ts
export interface OperationsMessage {
	type: 'operations'
	busy_reasons: BusyReasonCode[]
	operations: Operation[]
}

export interface OperationListResponse {
	operations: Operation[]
	busy_reasons: BusyReasonCode[]
}

export interface AllOperationsResponse {
	operations: Operation[]
	busy_reasons_by_server: Record<string, BusyReasonCode[]>
}

/** Response of every endpoint that starts an operation. Always 202, except for POST /servers. */
export interface OperationAcceptedResponse {
	operation: Operation
}
```

### 4.3 Creating a server

```ts
export type CreateServerContent =
	| {
			kind: 'loader'
			/** vanilla | paper | folia | purpur | leaf | fabric | velocity | neoforge | quilt | forge */
			loader: string
			game_version: string
			/** null = latest stable build of this loader for this game version. */
			loader_version: string | null
	  }
	| {
			kind: 'modpack_project'
			project_id: string
			version_id: string
	  }
	| {
			kind: 'modpack_upload'
			file_name: string
			file_size: number
	  }

export interface KnownPropertiesFields {
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
	known: KnownPropertiesFields
	custom?: Record<string, string>
}

export interface CreateServerRequest {
	name: string
	/** Only a panel administrator may set this to ≠ null. */
	owner_id: string | null
	memory_mib: number
	/** Only a panel administrator may set this to ≠ null. */
	port: number | null
	eula_accepted: boolean
	content: CreateServerContent
	properties: PropertiesFields
}

export interface CreateServerResponse {
	server_id: string
	operation: Operation
}
```

`KnownPropertiesFields` and `PropertiesFields` are literally Modrinth's types
(`api-client/src/modules/archon/types.ts:391-422`), because the wizard builds exactly this form
(`creation-flow-context.ts:524-541`).

### 4.4 Mappings in the provider

```ts
const PHASE_TO_SYNC: Record<OperationPhase, 'Analyzing' | 'InstallingLoader' | 'InstallingPack' | 'Addons'> = {
	analyzing: 'Analyzing',
	installing_loader: 'InstallingLoader',
	installing_pack: 'InstallingPack',
	addons: 'Addons',
}

const BUSY_MESSAGE: Record<BusyReasonCode, MessageDescriptor> = {
	installing: defineMessage({ id: 'servers.busy.installing', defaultMessage: 'Server is installing' }),
	syncing_content: defineMessage({ id: 'servers.busy.syncing-content', defaultMessage: 'Content sync in progress' }),
	backup_creating: defineMessage({ id: 'servers.busy.backup-creating', defaultMessage: 'Backup creation in progress' }),
	backup_restoring: defineMessage({ id: 'servers.busy.backup-restoring', defaultMessage: 'Backup restore in progress' }),
	deleting: defineMessage({ id: 'servers.busy.deleting', defaultMessage: 'Server is being deleted' }),
}

const toFileOperation = (op: Operation): FileOperation => ({
	id: op.id,
	op: op.kind,
	src: op.src ?? '',
	state: op.state,
	progress: op.progress,
	bytes_processed: op.bytes_processed ?? undefined,
	files_processed: op.files_processed ?? undefined,
	current_file: op.current_file ?? undefined,
})

const activeOperations = computed(() =>
	operations.value.filter((op) => op.kind === 'unarchive' && op.state !== 'cancelled').map(toFileOperation),
)
```

The four values in `PHASE_TO_SYNC` are Modrinth's spelling
(`api-client/src/modules/archon/types.ts:1160`) and must not differ — `InstallingBanner` compares
them verbatim (`InstallingBanner.vue:187-196`), and `ServerPanelAdmonitions.vue:179` checks for
`'Analyzing'`. Our API stays with lowercase and underscores all the same, because `phase` also hangs
off operations that never reach the banner.

Only `unarchive` becomes a `FileOperation`. Reason: `FileOperationAdmonition` is tailored to
extracting — the heading is fixed at "Extracting {source}" (`FileOperationAdmonition.vue:68-79`) and
the icon is `PackageOpenIcon` (`:11`). A backup operation in that list would read "Extracting".
Install operations go to `InstallingBanner`, backup operations to `BackupAdmonition`.

### 4.5 Error codes of an operation

`error.code` is stable; `error.message` is what `InstallingBanner` sees as
`ContentError.description`. Where Modrinth's banner translates a text anyway
(`InstallingBanner.vue:150-176`), we send exactly that text.

| `code` | `step` | `message` | Effect in the banner |
|---|---|---|---|
| `unsupported_version` | `modloader` | `this version is not yet supported` | translated message |
| `invalid_version` | `modloader` | `the specified version may be incorrect` | translated message |
| `loader_install_failed` | `modloader` | `internal error` | translated message |
| `modpack_no_primary_file` | `modpack` | `no primary file` | translated message |
| `modpack_install_failed` | `modpack` | `failed to install modpack` | translated message |
| `invalid_modpack` | `modpack` | our own text | raw text |
| `checksum_mismatch` | `download` | our own text | raw text |
| `upstream_unavailable` | `download` | our own text | raw text |
| `disk_full` | `filesystem` | our own text | raw text |
| `archive_corrupted` | `filesystem` | our own text | — (only `FileOperationAdmonition`) |
| `invalid_path` | `filesystem` | our own text | — |
| `payload_timeout` | `internal` | our own text | raw text |
| `panel_restarted` | `internal` | our own text | raw text |
| `interrupted_while_applying` | `filesystem` | our own text | raw text |
| `restore_interrupted` | `filesystem` | our own text | raw text |
| `cancelled_by_user` | `internal` | our own text | — |

### 4.6 Who locks what

The "locks" column is what the server enforces with `409 server_busy`. The "busy_reason" column is
what the interface sees — and because `busyReasons.length > 0` locks all power actions and all
writes across the board in the vendored interface (1.3), the two columns are identical on purpose.

| Kind | `busy_reason` | locks | cancellable | server may run |
|---|---|---|---|---|
| `server_create` | `installing` | everything | yes | — |
| `install_loader` | `installing` | everything | no | no |
| `install_modpack` | `installing` | everything | no | no |
| `repair_content` | `installing` | everything | no | no |
| `install_content` | `syncing_content` | everything | no | yes |
| `backup_create` | `backup_creating` | everything | yes | yes |
| `backup_restore` | `backup_restoring` | everything | yes | no |
| `unarchive` | **none** | nothing; further `unarchive` runs are queued | yes | yes |
| `server_delete` | `deleting` | everything | no | no |

The five kinds added in 4.1 are missing here on purpose: `update_content` behaves like
`install_content`, `update_modpack` and `change_game_version` like `install_modpack`, `reinstall`
like `install_loader`. For `install_java` nothing is decided: it is the only operation that touches
no user data and therefore may not have to lock at all. The area that owns the operation should
enter it; this table is the place for that.

Two points that need explaining:

**`unarchive` sets no busy reason.** Modrinth does the same: the busy reasons arise solely from
`status === 'installing'`, `isSyncingContent` and the backups
(`composables/server-manage-core-runtime.ts:108-128`, `composables/server-backups-queue.ts:92-111`).
If we made extracting a busy reason, the file manager would go dark during the extraction
(`files.vue:53`), and that is exactly the page on which you want to watch the progress. Instead,
several `unarchive` operations per server are serialized: the second one stays `queued`, which the
vendored component correctly shows as waiting (`FileOperationAdmonition.vue:7`).

**The install operations are not cancellable.** `InstallingBanner` has no cancel button; it knows
only "retry" on failure and "dismiss" (`InstallingBanner.vue:29-41`,
`ServerPanelAdmonitions.vue:214-221`). Offering a cancel that no vendored component can trigger
would be dead code. The emergency exit is restarting the panel, or deleting the server.

There is one exception, `server_create`: there a cancel **does** make sense, because the user can
have picked the wrong modpack and the server is not worth anything yet. It is triggered from **our
own** page (the creation flow), not from the banner.

### 4.7 Panel restart and cleanup

**Principle: nothing is resumed, except deleting.** Our operations have no checkpoints; resuming a
half-downloaded installer reliably would require a journal per operation, which we do not have and
will not build for operations that take a minute.

**At panel start**, in this order:

1. Every operation row in `queued` or `ongoing` is set to `failed`, with
   `error.code = "panel_restarted"`, `finished_at = now` and **no** `dismissed_at` — the user should
   see that something was broken off.
2. Exception: `server_delete` is **resumed**. Deleting is repeatable, and a half-deleted server has
   no value worth keeping.
3. Cleanup happens per kind (table below).
4. Every work directory `<serverdirectory>/.craftpanel-tmp/<op_id>/` that has no running operation
   belonging to it is deleted — orphans without a database row too.
5. Finished operations older than **seven days** are deleted. The audit log is a different table and
   stays.

**The work directory is the entire cleanup rule.** Every operation that creates files puts them
under `<serverdirectory>/.craftpanel-tmp/<op_id>/` first and only moves them into place at the end —
in the same file system, so with `rename`. The file manager hides `.craftpanel-tmp`. As long as
nothing has been moved, a crash leaves exactly nothing behind.

| Kind | Cleanup after a restart in mid-run |
|---|---|
| `server_create` | delete the work directory, reset the server directory to empty (it cannot contain anything of the user's), server to `status: "broken"`, `flows.intro: true` — the interface then shows the creation page again (`root.vue:135`, `ServerListing.vue:447`). Port and system user stay taken |
| `install_loader`, `install_modpack`, `repair_content` | delete the work directory. Server to `status: "broken"` — the jar can be half swapped, and a half-swapped loader starts into an incomprehensible crash. A retry or a reinstall heals it |
| `install_content` | delete the work directory. Every content file is downloaded to the end on its own and then moved — so a file is either fully there or not at all. The server stays `available` |
| `unarchive` | see below |
| `backup_create` | delete the archive that was started, delete the backup row. Nothing on the server changed |
| `backup_restore` | server to `status: "broken"` with `error.code = "restore_interrupted"`. The world directory may be half overwritten; being honest here is better than passing off a broken state as sound. The user restores again |
| `server_delete` | resume |

**The half-extracted archive, explicitly.** An `unarchive` has two stages, and the operation row
carries `applied_at` for it:

- **Before `applied_at`** — the extraction goes into `.craftpanel-tmp/<op_id>/`. A crash here leaves
  nothing behind: the directory is gone, the operation is `failed` with `panel_restarted`, and the
  target directory is untouched. This is the normal case, because this stage takes almost the whole
  running time.
- **After `applied_at`** — the extracted entries are moved into the target. That takes milliseconds,
  but with many entries it is not atomic. If the panel crashes exactly here, a subset lies in the
  target. **We do not delete those files.** The operation ends in `failed` with
  `error.code = "interrupted_while_applying"` and a message that says so. Reason: deleting the
  user's files because we are unsure is the worse mistake; the target of an extraction is a
  directory with other people's files in it.

The same split applies to `install_modpack` — only there the target is emptied beforehand anyway,
which is why a crash while applying simply leads to `broken`.

### 4.8 The "create a server" flow, complete

Modrinth's `onboarding.vue` is the same flow, only without the purchase in front of it
(`docs/PLAN.md:324-337`). At Modrinth the server exists before the wizard, because it was paid for;
with us it comes into being **with** the wizard. That is the only difference in the flow — and the
reason why we hang the wizard off the server list and not off a server detail page.

| # | Who | Call | Operation | Server state | in the list | startable |
|---|---|---|---|---|---|---|
| 1 | browser | — (the wizard opens) | — | does not exist | no | no |
| 2 | browser | `POST /api/v1/servers` | — | being created | no | no |
| 3 | panel | response `201` | `server_create`, `queued` | `installing` | **yes, with a spinner** | no |
| 4 | browser | `GET`/WS on `/servers/:id` | `queued` → `ongoing` | `installing` | yes | no |
| 4b | browser | `PUT …/payload` (only `.mrpack`) | stays `queued` | `installing` | yes | no |
| 5 | panel | — | `ongoing`, `analyzing` | `installing` | yes | no |
| 6 | panel | — | `ongoing`, `installing_loader` | `installing` | yes | no |
| 7 | panel | — | `ongoing`, `installing_pack` (modpack only) | `installing` | yes | no |
| 8 | panel | — | `done`, dismissed by itself | `available` | yes, normal | **yes** |
| 8' | panel | — | `failed` | `broken` | yes | no |

In detail:

**Step 1 — the wizard.** `CreationFlowModal` with `type="server-onboarding"`
(`onboarding.vue:63-75`) and our ten loaders in `available-loaders` — the prop is `string[]` and
takes them (`creation-flow-modal/index.vue:29`). **The matching version list, however, it does not
get from us**, see 5.5.7. Two steps come on top that Modrinth does not have: **memory** (the slider
stops at the budget) and **EULA** (`docs/PLAN.md:334-335`). The port is not asked for, the panel
assigns it (`docs/PLAN.md:333`); an administrator may set it. The server name comes from
`config.worldName` (`creation-flow-context.ts:140`), the world settings from
`config.buildProperties()` (`:524-541`). None of this leaves the browser up to here.

**Step 2 — the one call.** `onCreate` (`onboarding.vue:284`) sends `POST /api/v1/servers`. Unlike at
Modrinth it is **one** call instead of two (`installContent` plus `endIntro`,
`onboarding.vue:272,353`), because with us creating and setting up do not fall apart. `flows.intro`
is `false` from the start.

**Step 3 — the response, and what is already finished.** Inside the request the following is done,
all in one transaction (2.7): the budget check, taking the port, the ULID, the database row, the
server directory, the operation row. No network traffic, and no `useradd` — the system user has
existed ever since the panel user has (`docs/PLAN.md:138-141`). From the response onwards the server
is visible through `GET /api/v1/servers` and the WebSocket is reachable. **This is where the server
appears in the list** — with `status: "installing"`. Turning that into the `is-provisioning` prop is
**our** list page's job (`docs/PLAN.md:75`): `ServerListing` derives nothing, `isProvisioning` is an
optional prop of its own (`ServerListing.vue:434`), and at Modrinth it comes from billing
(`index.vue:185` with `:916`: an unpaid subscription plus an open charge), not from `status`. If our
page sets it, we get a spinner over the icon (`ServerListing.vue:30-33`) and the hint line "Please
wait while we set up your server." (`:109-110`, `:317-320`) for free; if it does not set it, the
server looks like every other one.

**Step 4 — navigation and socket.** The browser goes to `/servers/:id`, the detail page opens the
socket and immediately gets the snapshot from 3.1 with `busy_reasons: ["installing"]`. The provider
then sets `server.status = 'installing'`, `InstallingBanner` appears, and all power and write
actions are locked (`use-server-power-action.ts:39-44`).

**Step 4b — only for the uploaded modpack.** The operation stays `queued` while the browser sends
`PUT …/payload`. The progress comes from the XHR and fills `uploadState` → `UploadAdmonition`
appears (`ServerPanelAdmonitions.vue:223-232`). It sits **below** the install banner, not above it:
the stack is sorted by `priority` (`:282`), and the banner has 0 (`:218`), the upload 2 (`:229`). As
long as the phase is `analyzing`, the banner is invisible anyway (`:179`) and the upload stands
alone. Modrinth makes the same intermediate step, only with a state of its own instead of an
operation (`onboarding.vue:291-323`).

**Steps 5 to 7 — the work.** Progress is throttled to one message per second.

- `analyzing` (0 to 0.05): resolve the version, fetch the download address and the checksum. **The
  banner is invisible in this phase** (`ServerPanelAdmonitions.vue:179`) — which is why it has to be
  short.
- `installing_loader` (0.05 to 0.70): download the jar into `.craftpanel-tmp/<op_id>/`, compare the
  checksum, run the installer for NeoForge, Forge and Quilt (`docs/PLAN.md:396-400`), then move it
  into place.
- `installing_pack` (0.70 to 0.95, modpack only): read the `.mrpack`, unpack the overrides, download
  the files of the pack list.
- At the end (0.95 to 1.00): write `eula.txt` — only because step 1 forced the checkbox —,
  `server.properties` from `properties`, `-Xmx` from `memory_mib`, the startup command from the
  loader definition.

**Step 8 — done.** Operation to `done`, `dismissed_at = finished_at` (reasoning in 5.4), server to
`available`, `busy_reasons` empty. The provider clears the banner away, the power buttons become
active. **From here on the server can be started** — and not a moment earlier, because
`busyReasons.length > 0` locks every power action until then.

**Step 8' — failed.** Operation to `failed` with `error`, server to `broken`, operation **not**
dismissed. The banner turns red, shows `error.message` (after the translation from 4.5) and offers
"retry" (only with `SETUP`) and "dismiss" (`InstallingBanner.vue:29-41`,
`ServerPanelAdmonitions.vue:212-221`). "Retry" is `POST …/retry` and creates a new `server_create`
with the same inputs. The port, the system user and the directory stay taken while that happens;
they are only freed with `DELETE /api/v1/servers/:id`.

---

## 5. Open questions and assumptions

### 5.1 `world_id` — the constant `"default"`

We have one world per server; Modrinth's shared building blocks want a `world_id` regardless:
`useServerBackupsQueue` only runs with `worldId != null` (`composables/server-backups-queue.ts:23`),
and `installation.vue:246` and `content.vue` use `worldId.value!`.

**Proposal:** the constant `"default"`. The server area delivers
`serverFull.worlds = [{ id: "default", name: <servername>, is_active: true, … }]`, and `ctx.worldId`
is `ref("default")`. None of our endpoints takes a `world_id`; if somebody sends one, it is ignored.
That has to be decided in the server area — only the requirement stands here.

### 5.2 `useServerManageCoreRuntime` is not usable

`composables/server-manage-core-runtime.ts` sits in `composables/`, so in the part the plan lists as
adoptable — but it is tied firmly to Modrinth's client: `client.archon.sockets.safeConnect` (`:321`),
`client.kyros.files_v0.modifyOperation` (`:373`), `client.archon.servers_v0.getFilesystemAuth`
(`:385`). We write `provideModrinthServerContext` ourselves. That is no surprise — the plan budgets
200 to 400 lines per area — but it means that the 450 lines of this runtime belong to our work and
not to the adopted library. **Belongs in the server area's inventory.**

### 5.3 `ServerPanelAdmonitions` is wired up too

The component that stacks all operations (`components/servers/admonitions/ServerPanelAdmonitions.vue`)
injects Modrinth's client (`:34`) and calls `client.archon.backups_queue_v1.ackCreate`, `ackRestore`,
`cancelCreate`, `cancelRestore`, `retry` and `backups_v1.delete` (`:293-346`). On top of that it
hangs off `useServerBackupsQueue`, which calls `backups_queue_v1.list()`
(`server-backups-queue.ts:22`).

So it is **not adoptable unchanged** without us rebuilding Modrinth's `backups_queue_v1` response
form byte for byte — exactly what `docs/PLAN.md:58-67` rejects.

**Recommendation:** an `OperationsAdmonitions.vue` of our own, about 120 lines, that fills
`StackedAdmonitions` (`components/base/StackedAdmonitions.vue`) with the four displays. The four are
suitable for it:

| Component | Dependency |
|---|---|
| `InstallingBanner.vue` | props and events only, no injection |
| `BackupAdmonition.vue` | props and events only, no injection (`:1-14`) |
| `FileOperationAdmonition.vue` | injects only `ctx.dismissOperation` (`:64,111`) |
| `UploadAdmonition.vue` | injects only `ctx.uploadState` (`:58-60`) |

The stacking and priority rules can be read off `ServerPanelAdmonitions.vue:182-283` and are short.
**The server area has to decide this too**, because it is its page the stack hangs on.

### 5.4 What I had to decide

| Decision | Reason | Alternative that was rejected |
|---|---|---|
| `state: 'failed'`, not `'error'` | `FileOperationAdmonition.vue:3,94` checks `startsWith('fail')` | `error` — would have swallowed the red state |
| `progress` in 0…1 | `Admonition.vue:148` and `FileOperationAdmonition.vue:5` work in 0…1; only `InstallingBanner` wants 0…100 and divides itself | 0…100 — two conversions instead of one |
| `busy_reasons` comes from the server | The server enforces the lock with `409`; if the browser derived it, the grayed-out button and the refusal would drift apart | derive it in the browser from the operation list |
| Full snapshots instead of differences | Modrinth's own pattern (`WSFilesystemOpsEvent.all`); no ordering problems, no catching up after a dropped connection | delta messages |
| The row before the work at `POST /servers` | solves the socket question completely, without a second socket | a panel-wide socket; or an operation id without a server and moving it over later |
| A successful `server_create` dismisses itself | `InstallingBanner` has no success state at all; at Modrinth it simply disappears (`root.vue:658-672`). A green box would be new construction | leave the success standing |
| `retry` creates a new operation | Modrinth wires the button to `repair`, so to a fresh run (`root.vue:1093`) | reset the same operation |
| Dismissing on the server | survives a page reload | in the browser only, which Modrinth does in addition |
| The upload goes to the operation | one identifier less, the cleanup rule applies without an addition | a staging endpoint of its own with an id of its own |
| `unarchive` without a busy reason | otherwise the file manager locks itself out (`files.vue:53`) | set a busy reason |
| No resuming after a restart | no checkpoints available; operations take a minute | resume with a journal |

### 5.5 What somebody else has to decide

1. **`status: "deleting"`** — Modrinth's `Status` knows only `installing | broken | available | suspended`
   (`api-client/src/modules/archon/types.ts:581`). We need a fifth value, or we have to show the
   deletion differently. Where it hurts is the **type**, not the flow: `ServerListing.vue:419`
   declares the prop as `Archon.Servers.v0.Status`, and a fifth value is a compile error there. At
   runtime nothing bad happens — the three branches in `ServerListing.vue:451,463,476` only check
   for `'suspended'` and for the component's own prop `isProvisioning`, and unknown values fall
   through silently. Either we widen the union in the vendor copy, or the deletion shows itself
   through `is-provisioning` plus a hint line of our own. **Server area.**
2. **The backup queue** — whether the backups area offers a queue list of its own or feeds its
   display entirely from `GET …/operations`. The mapping in 1.6 makes both possible; two sources for
   the same state would be the mistake. **Backups area.**
3. **Where the installer's output goes** — NeoForge, Forge and Quilt start an installer whose output
   you want to see (`docs/PLAN.md:396-400`). My proposal: into the normal console stream, as
   Modrinth hints at (`root.vue:1210` clears the console on a reinstall). Then no separate endpoint
   for operation logs is needed. **Console area.**
4. **Audit log** — every operation should produce an entry (Modrinth knows `server_repaired`,
   `layouts/wrapped/hosting/manage/[id]/access/audit-log-utils.ts:42`). Whether that is derived from
   the operations table or written separately is for the accounts area to decide.
5. **The maximum number of concurrent operations across all servers** — per server everything is
   serialized, but ten concurrent modpack installations on ten servers saturate the line. I propose
   a panel-wide queue with a width the administrator can set (default: 2). Affects the administrator
   settings. **Not settled.**
6. **Panel-wide operations** — the panel updating itself and creating a system user without a server
   do not fit into this model, because every operation has a `server_id`. Today there is none; as
   soon as there is one, it needs either a `server_id: null` and a delivery path for it, or the
   thing is not carried as an operation. **Deliberately open.**
7. **Where the wizard gets its loader and version lists from.** This area defines the creating, but
   no catalog — and without a catalog, `422 unsupported_version` is the only feedback the user ever
   sees. Measured against `CustomSetupStage.vue:344-375`: for `vanilla` the game versions come from
   the `tags` provider, for `paper` and `purpur` from hard-wired queries to PaperMC and Purpur
   (`creation-flow-context.ts:375-395`), and for everything else from `ctx.loaderVersionsCache[loader]`,
   filled from Modrinth's launcher-meta. For our three additional loaders — **Folia, Leaf,
   Velocity** — launcher-meta knows nothing, the list stays empty (`:367`: `if (!manifest) return []`),
   and the choice is dead. The seam for it exists: `getLoaderManifest` is a resolver function that
   can be injected (`creation-flow-context.ts:237,264`). So an endpoint is needed that delivers a
   launcher-meta-shaped manifest response. Who builds it and where it is documented is open —
   **proposal: the settings area**, where `POST …/install` already carries the same question.
   **Not settled.**
