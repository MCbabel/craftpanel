# Backups — the interface contract

As of 2026-08-12. Area P5 (`docs/PLAN.md:451`).

All paths without a prefix are relative to `vendor/modrinth/`.

## Decisions at a glance

| Question | Answer | where it is argued |
|---|---|---|
| What is backed up? | **the whole server directory** minus the exclusion list, not only the world | A2 |
| Hold the server still? | **create: no** (`save-off` / `save-all flush` / `save-on`), **restore: yes, enforced** | A3 |
| Automatic backups? | **yes**, interval + `keep_last` per server, off by default | A1 |
| A queue with `ack`/`cancel`? | **no multi-item stack**, but operation records with `ack`, `cancel`, `retry` — the interface calls them | A5 |
| Storage location | `/var/lib/<panel>/backups/<server-id>/`, **outside** the server directory, `craftpanel:craftpanel 0700` | A6 |
| Does it count against the user's limit? | against `max_backups` per server (a count, at most 50); a per-user byte budget is missing from the plan → F3 | A7 |
| One world per server | `world_id` **never** goes on the wire; the provider sets the constant `"default"` | 1.8 |
| Who owns a restored directory? | **unresolved** — the panel writes as `craftpanel`, the server runs as `craft-<id>`, the helper cannot `chown` | A3, F7 |
| Where do the bytes live? | `local` **or** `drive` per server (contract 22, `docs/DRIVE.md`); a row keeps its location forever | 4 |

---

## 1. The provider contract

### 1.1 There is no `provideBackupManager`

Unlike the file manager (`ui/src/layouts/shared/files-tab/providers/file-manager.ts`) and the
console, the backups area has **no** `provide` contract. The contract has three parts instead:

1. **The client module `client.archon.backups_queue_v1`** — a TypeScript class contract in
   `api-client/src/modules/archon/backups-queue/v1.ts:4-113`, plus `backups_v1.rename` in
   `api-client/src/modules/archon/backups/v1.ts:100-112`. It is fetched through
   `injectModrinthClient()` (`ui/src/providers/api-client.ts:5`).
2. **The WebSocket event `WSBackupProgressEvent`**
   (`api-client/src/modules/archon/types.ts:1030-1038`), which flows into the state through
   `handleWsBackupProgress` (`ui/src/composables/server-backups-queue.ts:71-86`).
3. **`AppBackupContext`** (`ui/src/providers/app-backup.ts:3-5`) — the one-method contract
   `createBackup(): Promise<void>` for environments **without** a server context. Modrinth's desktop
   app fills exactly that one (`/root/ref-modrinth/apps/app-frontend/src/providers/instance-backup.ts:9-24`).
   **Not** usable for us: `useInlineBackup` takes the `AppBackupContext` branch only when
   `injectModrinthServerContext(null)` returns **null**
   (`ui/src/layouts/shared/content-tab/composables/use-inline-backup.ts:13-21`). We do provide the
   server context (the console, files and content need it), so we always land in the client branch.
   **That is the central insight of this document.**

**Who calls the contract even though it does not sit under `wrapped/`** — that is, code we adopt
according to the plan:

| File | calls | Lines |
|---|---|---|
| `ui/src/composables/server-backups-queue.ts` | `backups_queue_v1.list`, `WSBackupProgressEvent` | 24, 71 |
| `ui/src/layouts/shared/content-tab/composables/use-inline-backup.ts` | `create`, `list`, `cancelCreate` | 130, 154, 163 |
| `ui/src/components/servers/admonitions/ServerPanelAdmonitions.vue` | `ackCreate`, `ackRestore`, `cancelCreate`, `cancelRestore`, `retry`, `backups_v1.delete` | 295, 301, 321, 323, 329, 344 |
| `ui/src/components/servers/backups/BackupCreateModal.vue` | `create` | 108-110 |
| `ui/src/components/servers/backups/BackupRenameModal.vue` | `backups_v1.rename` | 89-93 |
| `ui/src/components/servers/backups/BackupRestoreModal.vue` | `restore` | 83-87 |
| `ui/src/components/servers/backups/BackupItem.vue` | builds the download URL itself | 116 |

`InlineBackupCreator.vue` is used in **ten** places (counted: `<InlineBackupCreator` in `ui/src`):
deleting content, bulk update, modpack update, unlinking, reinstall, the dependency warning
(`ContentDependencyWarningModal.vue`), ZIP upload by URL, incompatible content, content comparison
(`ContentDiffModal.vue`) and the reset step in the creation wizard
(`components/flows/creation-flow-modal/components/FinalConfigStage.vue`), among them
`ui/src/layouts/shared/content-tab/components/modals/ConfirmDeletionModal.vue:17` and
`ui/src/layouts/shared/files-tab/components/modals/FileUploadZipUrlModal.vue:57`. So the backups
area is **not a page of its own but a cross-cutting dependency**.

**Consequence for us:** we write an adapter that implements the **ten** methods of
`backups_queue_v1` (`list`, `create`, `ackCreate`, `cancelCreate`, `ackRestore`, `cancelRestore`,
`delete`, `deleteMany`, `restore`, `retry`) plus `backups_v1.rename` and `backups_v1.delete` against
our endpoints. We rebuild the page `layouts/wrapped/hosting/manage/backups.vue` ourselves from
`components/servers/backups/*`; the four modals and `BackupItem.vue` are adopted unchanged.

**How the adapter gets in — not the way you would think.** `injectModrinthClient()` returns the one
`AbstractModrinthClient` that `provideModrinthClient` was given, and out of it the interface fetches
25 further modules (counted in `ui/src`: `client.labrinth.*`, `client.kyros.*`, `client.archon.*`).
Replacing `archon.backups_queue_v1` **on a real client fails**: `abstract-client.ts` creates every
module as a pure getter with `configurable: false`, and the API namespaces as
`writable: false, configurable: false`. Measured in Node:

- `client.archon.backups_queue_v1 = adapter` → `TypeError: Cannot set property … which has only a getter`
- `Object.defineProperty(client.archon, 'backups_queue_v1', …)` → `TypeError: Cannot redefine property`
- `client.archon = …` → `TypeError: Cannot assign to read only property 'archon'`
- a `Proxy` around the client whose `get` trap returns a different `archon` → `TypeError`, because
  the proxy invariant forces the same value for non-writable, non-configurable data fields

What does work: laying a **shim of our own** over the real client.

```ts
const shim = Object.create(realClient)
Object.defineProperty(shim, 'archon', {
  value: { ...realClient.archon, backups_queue_v1: adapter, backups_v1: legacyAdapter },
  enumerable: true,
})
provideModrinthClient(shim)
```

The spread evaluates the getters and freezes the remaining archon modules as ordinary fields,
`sockets` and `sync` too, which the concrete client attaches afterwards (`platform/generic.ts:33-44`).
`labrinth` and `kyros` the shim inherits through the prototype chain. These four lines are
cross-area: every further area that replaces a `client.*` module hooks into the same place.

### 1.2 `BackupQueueBackup` — field by field

Declaration: `api-client/src/modules/archon/types.ts:896-904`.

| Field | Type | Origin with us | actual use |
|---|---|---|---|
| `id` | `string` | ULID of the backup from `backups.id` | list key `backups.vue:137`, "copy id" `BackupItem.vue:142`, the target of every endpoint |
| `name` | `string` | user input, 1–128 characters | display `BackupItem.vue:216`, duplicate-name lock `BackupCreateModal.vue:121-129` |
| `created_at` | `string` | RFC 3339 UTC, the moment of **queuing**, not of finishing | grouping into "Just now/Today/Yesterday" `backups.vue:493-511`, sorting `server-backups-queue.ts:32-36`, display `BackupItem.vue:227,267` |
| `status` | `'pending' \| 'in_progress' \| 'timed_out' \| 'error' \| 'done'` | state of the **most recent operation of any kind**, not only of the `create`; `types.ts:851`. See the box below. | filter on `'done'` `backups.vue:398`, completion detection `use-inline-backup.ts:100-102`, delete path `backups.vue:695-697`, **busy detection** `server-backups-queue.ts:55-69` |
| `locked` | `boolean` | **always `false`** | **read nowhere** — only `/root/ref-modrinth/packages/ui/src/stories/servers/BackupItem.stories.ts:41` sets it (the stories are not vendored along). We deliver it because the type demands it, and we never use it. |
| `automated` | `boolean` | `true` for scheduled backups, `false` for manual ones and for safety backups taken before a restore | icon `BackupItem.vue:84-89`, badge "Auto" `BackupItem.vue:219-223`, filter pills `backups.vue:359-362,406` |
| `history` | `BackupQueueOperation[]` | all operations of this backup, **newest first** | author `BackupItem.vue:71-76`, `backups.vue:465-472`; `history[0]` counts as "the last operation" `ServerPanelAdmonitions.vue:138` — **the order is mandatory, not cosmetics** |

On top of that we deliver **`size_bytes: number`** (0 while it is not finished). Not a Modrinth
field, but needed for the space display and the quota; extra fields do not bother the interface
(`BackupItem.vue:306-308` shows the raw object in debug mode anyway).

**`status` has to flip for a restore as well.** The busy reasons do not arise from
`active_operations` alone, but from the **cross product** of operation and backup state:

```ts
const hasRunningRestore = computed(() =>
  activeOperations.value.some(
    (o) => o.operation_type === 'restore' &&
           backupById.value.get(o.backup_id)?.status === 'in_progress'),
)                                          // server-backups-queue.ts:63-69
```

Before the operation, the backup being restored is `done`. If we derived `status` only from the
`create` operation, it would stay that way. Then `hasRunningRestore` would be **always false**,
`busyReasons` would stay empty (`:102-108`), and with it everything that hangs off it would fall:
locking the settings, content and files pages, `backupRestoreDisabled` (`backups.vue:609-611`),
`backupCreationDisabled` (`:629-631`) and the basis of F1. So: `status = in_progress` as soon as an
operation of **any** kind is running on this backup, and back to the result of the most recent
operation afterwards.

The visible price: during a restore the source backup drops out of `completedBackups`
(`backups.vue:398`) and its row disappears from the list for the duration. That is Modrinth's
behavior and it fits L4.

### 1.3 `BackupQueueOperation` — field by field

Declaration: `types.ts:882-894`.

| Field | Type | Origin with us | actual use |
|---|---|---|---|
| `operation_type` | `'create' \| 'restore'` | kind of operation | `ServerPanelAdmonitions.vue:120,147`, author lookup `BackupItem.vue:73` |
| `operation_id` | `number \| null` | **the ULID of our operation row**, passed through in the adapter with `as unknown as number` (A4) | only handed back to `ackCreate`/`cancelCreate`/`ackRestore`/`cancelRestore` (`ServerPanelAdmonitions.vue:289-334`) and checked for `!= null` (`backups.vue:422-427`) — **never computed with, never sorted** |
| `state` | `'pending' \| 'ongoing' \| 'completed' \| 'cancelled' \| 'failed' \| 'timed_out'` | operation state, `types.ts:843-850` | banner text and color `BackupAdmonition.vue:49-70`, terminal-state check `ServerPanelAdmonitions.vue:139-140` |
| `scheduled_for` | `string` | RFC 3339, the moment of queuing | timestamp in the banner `ServerPanelAdmonitions.vue:133,153` |
| `started_at` | `string \| null` | start of the execution | **read nowhere** (checked: no hit in `ui/src`). We deliver it anyway, it costs nothing and answers "why is this hanging". |
| `completed_at` | `string \| null` | the end | timestamp in the terminal-state banner `ServerPanelAdmonitions.vue:153` |
| `has_parent` | `boolean` | `true` **only** for the `create` operation of a safety backup that came out of a `restore` | `hasActiveCreate` excludes such operations (`server-backups-queue.ts:49-51`) — if we set this wrongly, the interface blocks during every restore with "A backup is already queued or in progress" (`backups.vue:632-634`) |
| `error` | `string \| null` | error text, only for `failed`/`timed_out` | monospace line in the banner `BackupAdmonition.vue:97-99,277-278` |
| `should_prompt` | `boolean` | `true` as long as a terminal state has **not** been acknowledged (`ack`); see A5 | controls whether a finished operation still appears as a banner `ServerPanelAdmonitions.vue:139` |
| `synthetic_legacy` | `boolean` | **always `false`** | Modrinth's stopgap for backups from before the queue; on `true` the interface skips the `ack` (`ServerPanelAdmonitions.vue:151,289-292`). We have no legacy baggage. |
| `user_info` | `UserInfo \| null` | the panel user who triggered the operation; `null` for the schedule | author line `BackupItem.vue:229-251` |

### 1.4 `ActiveOperation` — field by field

Declaration: `types.ts:871-880`. This is the subset of operations with `state ∈ {pending, ongoing}`,
carried flat next to the list.

| Field | Origin | Use |
|---|---|---|
| `backup_id` | ULID of the backup it belongs to | mapping `server-backups-queue.ts:38-42`, `use-inline-backup.ts:155-157` |
| `operation_type` | as in 1.3 | `hasActiveCreate` / `hasActiveRestore` `server-backups-queue.ts:49-54` |
| `operation_id` | as in 1.3 | cancel `use-inline-backup.ts:152-163` |
| `has_parent` | as in 1.3 | `server-backups-queue.ts:50,59` |
| `scheduled_for` | as in 1.3 | banner timestamp `ServerPanelAdmonitions.vue:133` |
| `started_at` | as in 1.3 | unused |
| `synthetic_legacy` | always `false` | `ServerPanelAdmonitions.vue:131` |
| `user_info` | as in 1.3 | unused on this path |

The redundancy with `history` is intended and cheap: the same operation stands in both lists. The
subset is exactly `state ∈ {pending, ongoing}`. Otherwise `ServerPanelAdmonitions` shows a
permanent banner, because entries from `active_operations` are never dismissible (`:250`).

### 1.5 `UserInfo` — field by field

Declaration: `types.ts:861-865`.

| Field | Origin | Use |
|---|---|---|
| `id` | ULID of the panel user | the special case `id === 'support'` (`BackupItem.vue:79,178,186`) — **does not apply to us**, the "support staff" role is out per `docs/PLAN.md:94`. We never send `"support"`. |
| `username` | sign-in name | display `BackupItem.vue:249`, **and the profile link `/user/<username>` `BackupItem.vue:78-82`** → see gap L1 |
| `avatar_url` | `null` (we have no avatars) | `BackupItem.vue:184-188`; on `null` the `Avatar` colors itself from the name — nothing breaks |

### 1.6 The methods — one by one

| Method (source) | our endpoint | Note |
|---|---|---|
| `list(serverId, worldId)` `backups-queue/v1.ts:10-18` | `GET /api/v1/servers/{server_id}/backups` | `worldId` is discarded |
| `create(serverId, worldId, {name})` `:21-30` | `POST /api/v1/servers/{server_id}/backups` | the response must contain `{ id }` (`use-inline-backup.ts:130` destructures it) |
| `restore(serverId, worldId, backupId, {name})` `:94-104` | `POST /api/v1/servers/{server_id}/backups/{backup_id}/restore` | `name` is the name of the **safety backup** (`BackupRestoreModal.vue:78-87`) |
| `retry(serverId, worldId, backupId)` `:107-112` | `POST /api/v1/servers/{server_id}/backups/{backup_id}/retry` | |
| `delete(serverId, worldId, backupId)` `:69-78` | `DELETE /api/v1/servers/{server_id}/backups/{backup_id}` | |
| `deleteMany(serverId, worldId, backupIds)` `:81-91` | `POST /api/v1/servers/{server_id}/backups/bulk-delete` | |
| `cancelCreate(serverId, worldId, operationId)` `:41-46` | `POST /api/v1/servers/{server_id}/backup-operations/{operation_id}/cancel` | one endpoint for both kinds |
| `cancelRestore(...)` `:57-66` | likewise | |
| `ackCreate(serverId, worldId, operationId)` `:33-38` | `POST /api/v1/servers/{server_id}/backup-operations/{operation_id}/ack` | one endpoint for both kinds |
| `ackRestore(...)` `:49-54` | likewise | |
| `backups_v1.rename(serverId, worldId, backupId, {name})` `backups/v1.ts:100-112` | `PATCH /api/v1/servers/{server_id}/backups/{backup_id}` | Modrinth marks the method as "legacy", the interface uses it as the only rename path all the same |
| Download URL, assembled in the component: `` `https://${kyrosUrl}/modrinth/v0/backups/${id}/download?auth=${jwt}` `` `BackupItem.vue:116` | `GET /modrinth/v0/backups/{backup_id}/download` (alias) | see A8 and gap L2 |
| `backups_v1.delete` in `ServerPanelAdmonitions.vue:321` | `DELETE /api/v1/servers/{server_id}/backups/{backup_id}` | emergency path when `operation_id == null` — never with us |

Not served, because no adopted file calls them: `backups_v1.list`, `backups_v1.get`,
`backups_v1.create`, `backups_v1.restore`, `backups_v1.retry` (`backups/v1.ts:16,27,42,57,85`). The
adapter may put `throw new Error('not supported')` there. Dead as well: `WorldFull.backups`
(`types.ts:757`, marked deprecated in the type itself) and the only reader of that field,
`composables/server-backup.ts` — exported, but called nowhere in `ui/src`. We do not fill
`WorldFull.backups`.

Two behavior rules for the adapter that follow from the code and are easily missed:

- **Errors have to be `Error` objects whose `message` contains the string `429` on a 429.**
  `BackupCreateModal.vue:164` and `use-inline-backup.ts:136` check
  `error.message.includes('429')` in order to show "you are creating backups too quickly".
- **`list` has to deliver `history` sorted descending** (1.2).

### 1.7 The WebSocket event

`WSBackupProgressEvent` (`types.ts:1030-1038`), handled in `server-backups-queue.ts:71-86`:

| Field | Origin | Use |
|---|---|---|
| `event` | `'backup-progress'` | channel name |
| `id` | **the id of the backup**, not of the operation | key `${id}:${task}` `server-backups-queue.ts:73` |
| `task` | `'file' \| 'create' \| 'restore'` | `'file'` is discarded at once (`:72`) — we never send it |
| `state` | `'pending' \| 'ongoing' \| 'done' \| 'failed' \| 'cancelled' \| 'unchanged' \| 'damaged'` (`types.ts:1020-1027`) | only `'ongoing'` stores the progress, every other value clears it (`:75-79`); **every change of state triggers a reload of the list** (`:81-85`) |
| `progress` | `0.0`–`1.0` | bar width; `Admonition.vue:148` clamps to `[0,1]`, so it is a **fraction, not a percentage** |
| `start_time`, `finish_time` | optional | **read nowhere** — we do not send them |

We do not mirror this 1:1 but send `backup_progress` (section 3); the adapter renames two fields.
We never produce `unchanged` and `damaged`.

### 1.8 `world_id`

Four adopted places require a non-empty `worldId`: `server-backups-queue.ts:22-23` (the query is
disabled as long as it is `null`), `use-inline-backup.ts:117,148`,
`ServerPanelAdmonitions.vue:40`, `backups.vue:391-394` (otherwise the page would stay in the loading
state forever).

**Proposal:** the server context gives `worldId = ref("default")`, a fixed string, not a ULID, so
that it is visible at once that it addresses nothing. It **never** appears in a path, a body or a
WebSocket message of our API; the adapter swallows the argument. Modrinth's derivation
`worlds.find(w => w.is_active)?.id` (`root.vue:624-628`) falls away with no replacement.

### 1.9 What the contract wants and we do not deliver

- **L1 — the profile link.** `BackupItem.vue:78-82` links the author to `/user/<username>` as soon
  as `user_info` is set and `id !== 'support'`. We do not have that route. Either the accounts area
  creates a user page, or the link goes nowhere. No way out through `creator: null`, because then
  the line shows the text "Manual backup" instead of the name (`BackupItem.vue:253-259`).
- **L2 — download over HTTPS only.** `BackupItem.vue:116` hard-codes `https://` into the URL. A
  panel on `http://192.168.1.10:8080` gets a broken link. The way out: do not set `kyros-url` and
  `jwt`, then the menu item is disabled (`:117`). The download is then reachable only through our
  own endpoint.
- **L3 — `locked`.** Modrinth knows locked backups; no adopted component reads the field. We deliver
  a constant `false` and so we have no "protected from deletion" feature.
- **L4 — no progress in the backup list.** `BackupItem.vue` shows no bar; progress appears only in
  the banner through `ServerPanelAdmonitions`. So a running backup is practically invisible in the
  list (`backups.vue:398` filters out everything but `done`). That is Modrinth's behavior, and we
  adopt it.
- **L5 — cancelling a running restore is a dead button.** `BackupAdmonition.vue:93-95` shows
  "Cancel" for every operation in state `pending` or `ongoing`, so for the `restore` too, which we
  do not cancel per 2.8. The click lands in `ServerPanelAdmonitions.vue:336-339`, and that rethrows
  the error **without a notification**. The user presses, and nothing visibly happens. We do not
  change the component, so it stays that way; whoever wants to fix it has to touch `canCancel`.

### 1.10 What other areas have to deliver so that this one works

Four fields that the backups interface reads but does **not** get from our endpoints. They hang off
the server object and off the server context and belong there. They stand here so that nobody
overlooks them:

| What | Where the interface takes it from | What happens without it |
|---|---|---|
| `backup_quota`, `used_backup_quota` (`types.ts:610-611`) | `server.value.backup_quota` in `backups.vue:622-627` | The quota check is silently skipped; instead of the message "All 10 of your backup slots are in use" the user gets our `409 backup_limit_reached` only after the click. `backup_quota` **has to** carry `max_backups`. |
| `kyros-url`, `jwt` | props of `BackupItem`; Modrinth feeds them from `server.node?.instance` / `server.node?.token` (`backups.vue:163-164`, `types.ts:654-657`) | The menu item "Download" is permanently gray (`BackupItem.vue:117`). Modrinth's node concept is out per `docs/PLAN.md:93-94` — our rebuilt page therefore sets the two props directly to `location.host` and `"cookie"`, without a `node` in the server object. See A8. |
| `busyReasons` | The server frame has to put `useServerBackupsQueue(...).busyReasons` into the context as `extraBusyReasons`, the way `root.vue:753` does | No locking of the other pages during a backup, and F1 loses its basis. |
| `isServerRunning` | `BackupRestoreModal.vue:4,93` | The restore button would stay usable while the server runs; our `409 server_running` catches it, but only after the click. |

On top of that, a side effect of `backup_list_changed`: on every `backup.*` Modrinth invalidates
**two** queries: the backup list and the server details (`server-panel-sync.ts:220-223`). Because
`used_backup_quota` hangs off the server details, our receiver has to do the same, otherwise the
quota display shows stale numbers.

---

## 2. The endpoints

Common rules: session cookie, `Content-Type: application/json`, timestamps RFC 3339 UTC, error body
`{ "error": "<code>", "message": "<text>" }`.

Permissions (`ui/src/composables/server-permissions.ts:17,22` for the bits, `:96` for
`canManageBackups`):

- **Read** — `BASE_READ`
- **Create, rename, delete, restore, retry, cancel, acknowledge, schedule** — `BACKUPS`
- A panel administrator (`docs/PLAN.md:283`) may do everything on all servers.

In addition the following holds for every endpoint: `401 unauthorized` without a session,
`403 forbidden` without the permission, `404 not_found` for unknown server or backup ids. These
three are not repeated below.

### 2.1 List — `GET /api/v1/servers/{server_id}/backups`

Permission: `BASE_READ`. No parameters, no pagination — but only because two upper bounds keep the
response small, and those therefore have to be written down:

- **`max_backups` ≤ 50.** The administrator may set the value, but not beyond 50. Without a cap,
  "no pagination, `max_backups` caps it" would be circular reasoning.
- **`history` ≤ 20 operations per backup**, the oldest ones drop out. Otherwise the history keeps
  growing without bound with every `retry`; the interface reads only `history[0]` and the first
  `create` operation with `user_info` anyway (`backups.vue:465-472`).

That puts the response at 50 × 20 operations in the worst case, around 300 KB of JSON. If one of
the limits is raised later, this endpoint needs pagination, and that costs work in the interface:
`server-backups-queue.ts:20-28` knows only one query without parameters.

Response `200`:

```json
{
  "active_operations": [
    {
      "backup_id": "01JEXZ9K2QW8T7VN4M0P3RCB6D",
      "operation_type": "create",
      "operation_id": "01JEXZ9K2QW8T7VN4M0P3RCB6E",
      "has_parent": false,
      "scheduled_for": "2026-08-12T14:03:11Z",
      "started_at": "2026-08-12T14:03:12Z",
      "synthetic_legacy": false,
      "user_info": {
        "id": "01JD8F5Q0000000000000USER",
        "username": "max",
        "avatar_url": null
      }
    }
  ],
  "backups": [
    {
      "id": "01JEXZ9K2QW8T7VN4M0P3RCB6D",
      "name": "Before the nether rebuild",
      "created_at": "2026-08-12T14:03:11Z",
      "status": "in_progress",
      "locked": false,
      "automated": false,
      "size_bytes": 0,
      "history": [
        {
          "operation_type": "create",
          "operation_id": "01JEXZ9K2QW8T7VN4M0P3RCB6E",
          "state": "ongoing",
          "scheduled_for": "2026-08-12T14:03:11Z",
          "started_at": "2026-08-12T14:03:12Z",
          "completed_at": null,
          "has_parent": false,
          "error": null,
          "should_prompt": true,
          "synthetic_legacy": false,
          "user_info": {
            "id": "01JD8F5Q0000000000000USER",
            "username": "max",
            "avatar_url": null
          }
        }
      ]
    },
    {
      "id": "01JEXQ7A5B9C1D3E5F7G9H1J3K",
      "name": "Backup #3",
      "created_at": "2026-08-12T02:00:00Z",
      "status": "done",
      "locked": false,
      "automated": true,
      "size_bytes": 1476395008,
      "history": [
        {
          "operation_type": "create",
          "operation_id": "01JEXQ7A5B9C1D3E5F7G9H1J3L",
          "state": "completed",
          "scheduled_for": "2026-08-12T02:00:00Z",
          "started_at": "2026-08-12T02:00:00Z",
          "completed_at": "2026-08-12T02:04:37Z",
          "has_parent": false,
          "error": null,
          "should_prompt": false,
          "synthetic_legacy": false,
          "user_info": null
        }
      ]
    }
  ]
}
```

Order: `backups` descending by `created_at` (the interface does sort by itself,
`server-backups-queue.ts:32-36`, but our own page should be right without a detour), `history`
descending by `scheduled_for`.

### 2.2 Create — `POST /api/v1/servers/{server_id}/backups`

Permission: `BACKUPS`.

Request:

```json
{ "name": "Before the nether rebuild" }
```

`name`: 1–128 characters after `trim`. The input fields cap at 48 (`BackupCreateModal.vue:12`,
`BackupRenameModal.vue:12`), but the name of the safety backup is cut to 92 characters
(`BackupRestoreModal.vue:78-81`) and created over the same path. **A limit of 48 in the backend
would smash every restore with a long backup name.** The backend allows duplicate names; the
interface **prevents** them when creating and renaming by hand: `nameExists` (comparison trimmed
and case-insensitive) disables the button, not merely a warning (`BackupCreateModal.vue:121-129`,
`BackupRenameModal.vue:104-119`). So duplicate names arise with us only over two paths that run past
this modal: the safety backup from 2.6 and the schedule. Both are allowed to produce them, which is
why the backend does not check.

Response `202`:

```json
{
  "id": "01JEXZ9K2QW8T7VN4M0P3RCB6D",
  "name": "Before the nether rebuild",
  "created_at": "2026-08-12T14:03:11Z",
  "status": "pending",
  "locked": false,
  "automated": false,
  "size_bytes": 0,
  "history": [
    {
      "operation_type": "create",
      "operation_id": "01JEXZ9K2QW8T7VN4M0P3RCB6E",
      "state": "pending",
      "scheduled_for": "2026-08-12T14:03:11Z",
      "started_at": null,
      "completed_at": null,
      "has_parent": false,
      "error": null,
      "should_prompt": true,
      "synthetic_legacy": false,
      "user_info": {
        "id": "01JD8F5Q0000000000000USER",
        "username": "max",
        "avatar_url": null
      }
    }
  ]
}
```

Errors:

| Status | `error` | when |
|---|---|---|
| `400` | `invalid_name` | empty after `trim`, longer than 128, or contains control characters (`\r`, `\n`, `\0`) — see 2.10 |
| `409` | `backup_operation_in_progress` | a `create` or `restore` is already running on this server |
| `409` | `backup_limit_reached` | `max_backups` reached — the interface catches this beforehand (`backups.vue:621-628`), the server checks anyway |
| `409` | `server_installing` | the server is still being set up |
| `429` | `rate_limited` | more often than once per 60 s; header `Retry-After` in seconds |
| `507` | `disk_full` | free space < estimated size × 1.1 |

```json
{ "error": "rate_limited", "message": "The next backup is possible in 43 seconds at the earliest." }
```

**Estimated size** means: the sum of the file sizes in the server directory minus the exclusion list
from A2, uncompressed. That is deliberately too high: the zstd file comes out smaller, and refusing
too early is better than ending up with a full disk.

**The check and the insert are one write.** `backup_operation_in_progress`, `backup_limit_reached`
and `rate_limited` all three check a state that the same call changes right afterwards. Two
concurrent requests — two browser windows, or two of the ten modals from 1.1 — would otherwise both
get through and create two operations on the same server, which is exactly what A5 is meant to rule
out. So: one SQLite transaction around the check and the `INSERT`, and a unique partial index on
"one open operation per server" as the second seam. The same holds for 2.6 and 2.7.

### 2.3 Rename — `PATCH /api/v1/servers/{server_id}/backups/{backup_id}`

Permission: `BACKUPS`.

```json
{ "name": "Base finished" }
```

Response `200`: the full backup object as in 2.1.

Errors: `400 invalid_name`; `409 backup_operation_in_progress` as long as an operation is running on
this backup.

### 2.4 Delete — `DELETE /api/v1/servers/{server_id}/backups/{backup_id}`

Permission: `BACKUPS`. Response `204`, no body. Deletes the file and the row at once; no trash can
(the modal says "Deletion is permanent", `BackupDeleteModal.vue:125-127`).

Errors: `409 backup_operation_in_progress` — running operations are cancelled through 2.8, not
through deleting. The interface already keeps the two paths apart itself (`backups.vue:695-724`).

### 2.5 Bulk delete — `POST /api/v1/servers/{server_id}/backups/bulk-delete`

Permission: `BACKUPS`.

```json
{ "backup_ids": ["01JEXQ7A5B9C1D3E5F7G9H1J3K", "01JEXR8B6C0D2E4F6G8H0J2K4M"] }
```

Field name from `DeleteManyBackupRequest` (`types.ts:867-869`).

Response `200`:

```json
{
  "deleted": ["01JEXQ7A5B9C1D3E5F7G9H1J3K"],
  "failed": [
    {
      "id": "01JEXR8B6C0D2E4F6G8H0J2K4M",
      "error": "backup_operation_in_progress",
      "message": "A restore is running on this backup right now."
    }
  ]
}
```

Partial success is explicitly allowed: deleting files one by one can fail one by one. The interface
does not read the body (`backups.vue:668-688` only checks whether the promise was broken) and
reloads afterwards, so whatever is left over becomes visible. `400` if `backup_ids` is empty or has
more than 100 entries.

### 2.6 Restore — `POST /api/v1/servers/{server_id}/backups/{backup_id}/restore`

Permission: `BACKUPS`. **The server has to be stopped** (A3).

```json
{ "name": "Before restoring \"Base finished\"" }
```

`name` is required and names the **safety backup** that is created before the overwrite
(`BackupRestoreModal.vue:78-87`). It gets `automated: false`, its `create` operation gets
`has_parent: true` (1.3) and `should_prompt: false`. Otherwise two success banners would stand next
to each other after every restore, "Backup finished (Before restoring …)" and "Restore finished".
The restore banner reports the outcome.

Response `202`:

```json
{
  "restore_operation_id": "01JEY0A1B2C3D4E5F6G7H8J9K0",
  "safety_backup": {
    "id": "01JEY0A1B2C3D4E5F6G7H8J9K1",
    "create_operation_id": "01JEY0A1B2C3D4E5F6G7H8J9K2"
  }
}
```

Flow: create the safety backup → unpack only once that has succeeded. If the safety backup fails,
**nothing is restored**; the `restore` operation goes to `failed` with
`error: "safety backup failed: <text>"`.

Errors:

| Status | `error` | when |
|---|---|---|
| `409` | `server_running` | the server is running (the interface does lock the button, `backups.vue:606-608`, `BackupRestoreModal.vue:92-94`, but there is no relying on it) |
| `409` | `backup_not_restorable` | `status != "done"` |
| `409` | `backup_operation_in_progress` | another operation is running |
| `409` | `backup_limit_reached` | the safety backup no longer fits into the quota |
| `507` | `disk_full` | not enough space for the safety backup + unpacking |

### 2.7 Retry — `POST /api/v1/servers/{server_id}/backups/{backup_id}/retry`

Permission: `BACKUPS`. Creates a **new** operation of the same kind as the most recent failed one
(`ServerPanelAdmonitions.vue:342-347`). For `create` the broken file is cleared away first and the
same backup row is reused: the id stays, so that the banner does not jump.

**For `restore` there is no name for a second safety backup.** The call has no body
(`backups-queue/v1.ts:107-112`), the interface asks for nothing. So a retry creates **no** new safety
backup: it keeps using the one from the first attempt, provided that one is `done`. If it is not
`done` — that is, the first attempt already failed on it — then the retry is a full restore with a
new safety backup, and its name is `Before restoring "<name of the backup>"`, cut to 92 characters
as in `BackupRestoreModal.vue:78-81`. Without this ruling there would be either a restore without a
net, or one more copy in the quota with every click.

Response `202`:

```json
{ "operation_id": "01JEY0B2C3D4E5F6G7H8J9K0L1", "operation_type": "create" }
```

Errors: `409 nothing_to_retry` when `history[0].state` is not `failed` or `timed_out`;
`409 server_running` on a `restore` retry; `409 backup_operation_in_progress`.

### 2.8 Cancel — `POST /api/v1/servers/{server_id}/backup-operations/{operation_id}/cancel`

Permission: `BACKUPS`. Serves `cancelCreate` **and** `cancelRestore` (`backups-queue/v1.ts:41,57`).
The kind is in the operation row, the caller does not have to send it along.

Response `204`.

Effect:

- `create` in state `pending`: operation to `cancelled`, the backup row disappears.
- `create` in state `ongoing`: the copying is aborted, the partial file deleted, the operation
  `cancelled`, the backup row disappears. If the server was held still for it (`save-off`), `save-on`
  is sent in any case.
- `restore` in state `pending`: cleanly cancellable.
- `restore` in state `ongoing`: **not cancellable**, `409 not_cancellable`. A half-unpacked server
  directory is worse than an unpack carried to the end. The interface does not know that and shows
  the button anyway. See L5.

Further errors: `409 not_cancellable` when the operation already has a terminal state. That case is
more common than it sounds: between the drawing of the banner and the click the operation may have
finished. 2.2 applies here as well: state check and state change in one transaction, otherwise you
cancel an operation that has just run through.

### 2.9 Acknowledge — `POST /api/v1/servers/{server_id}/backup-operations/{operation_id}/ack`

Permission: `BACKUPS`. Serves `ackCreate` and `ackRestore` (`backups-queue/v1.ts:33,49`). Sets
`should_prompt = false`; the banner disappears for good, even after a page reload. Response `204`.
Applied to an operation that has already been acknowledged: `204` as well (idempotent). The
interface also calls it when dismissing everything at once
(`ServerPanelAdmonitions.vue:364-380`).

Errors: `409 not_acknowledgeable` when the operation is still `pending` or `ongoing`.

### 2.10 Download — `GET /api/v1/servers/{server_id}/backups/{backup_id}/download`

Permission: `BACKUPS` (the file contains the entire server content, `BASE_READ` is not enough).

Response `200`, `Content-Type: application/zstd`, `Content-Length` set. No JSON body.

`Content-Disposition: attachment; filename="<slug>-<created_at>.tar.zst"; filename*=UTF-8''<percent-encoded>`.
`<name>` must **not** go raw into the header: it is up to 128 characters of free user input, and a
quotation mark or a line break in it either breaks the header apart or slips a second one in. So:
`<slug>` is `<name>`, reduced to `[A-Za-z0-9._-]`, everything else to `-`, cut to 64 characters,
empty → `backup`. The full name goes percent-encoded into `filename*`. Control characters are
rejected by 2.2 already; this here is the second seam, because the first one may change later.

Errors: `409 backup_not_downloadable` when `status != "done"`.

### 2.11 Download, compatibility alias — `GET /modrinth/v0/backups/{backup_id}/download`

The same response as 2.10. It exists solely because `BackupItem.vue:116` assembles the URL rigidly
and we adopt the component unchanged (A8). The server id is not in the path: the backup id is a
ULID and globally unique, and the server is looked up from it. The parameter `?auth=` is
**ignored**; authorization goes through the session cookie, which the browser sends along on the
same origin anyway. That way no secret ends up in the history.

So that the menu item does not stay gray (`BackupItem.vue:117`), the component needs `kyros-url` and
`jwt`. Both are **props**, not adapter values: `backups.vue:163-164` takes them from
`server.node?.instance` and `server.node?.token`. Our rebuilt page sets them directly to
`location.host` and `"cookie"`. That is the only way that gets by without a change to
`BackupItem.vue`, so our server object does not have to carry a `node` on top.

This one path lies **outside `/api/v1`**. That is a deliberate exception to the area rule, it stands
only here, it has no state of its own and no permission of its own.

### 2.12 Read the schedule — `GET /api/v1/servers/{server_id}/backups/schedule`

Permission: `BASE_READ`.

```json
{
  "enabled": true,
  "interval_hours": 24,
  "hour_utc": 4,
  "keep_last": 3,
  "next_run_at": "2026-08-13T04:00:00Z",
  "last_run_at": "2026-08-12T04:00:00Z",
  "last_status": "completed",
  "last_error": null
}
```

`last_status`: `"completed" | "failed" | "timed_out" | "skipped_unchanged" | "skipped_limit" | null`.

### 2.13 Set the schedule — `PUT /api/v1/servers/{server_id}/backups/schedule`

Permission: `BACKUPS`.

```json
{ "enabled": true, "interval_hours": 24, "hour_utc": 4, "keep_last": 3 }
```

Limits: `interval_hours` 1–168; `hour_utc` 0–23, evaluated only when `interval_hours % 24 == 0`;
`keep_last` 1–50 and ≤ `max_backups`. Response `200` with the same body as 2.12, plus the newly
computed `next_run_at`.

Errors: `400 invalid_schedule` with plain text in `message`.

### 2.14 Count

Twelve endpoints plus one compatibility alias (2.11).

---

## 3. WebSocket messages

Channel: the one socket per server, `/api/v1/servers/{server_id}/ws`. Two messages from this area,
both from the server to the client. There is no client→server message for backups; everything that
triggers something goes over HTTP.

### 3.1 `backup_progress`

```json
{
  "type": "backup_progress",
  "backup_id": "01JEXZ9K2QW8T7VN4M0P3RCB6D",
  "operation_id": "01JEXZ9K2QW8T7VN4M0P3RCB6E",
  "operation": "create",
  "state": "ongoing",
  "progress": 0.42
}
```

- `operation`: `"create" | "restore"`. Never `"file"` — the interface discards that at once
  (`server-backups-queue.ts:72`).
- `state`: `"pending" | "ongoing" | "done" | "failed" | "cancelled"`. Deliberately Modrinth's
  WebSocket vocabulary (`types.ts:1020-1027`) and **not** the operation vocabulary from 1.3, so that
  the adapter only renames two fields (`backup_id`→`id`, `operation`→`task`) and translates nothing.
  We never produce `"unchanged"` and `"damaged"`.
- `progress`: a fraction from `0.0` to `1.0`, from bytes copied ÷ total bytes.

Send rate: **at once** on every change of state, and at most every 500 ms while `ongoing`. Every
change of state leads to a reload of the list in the interface
(`server-backups-queue.ts:81-85`), which is why the terminal state is the most important message of
all.

### 3.2 `backup_list_changed`

```json
{ "type": "backup_list_changed" }
```

After creating, renaming, deleting, bulk deleting, `ack` and the cleanup by the retention rule.
Without a payload: the receiver reloads 2.1 **and the server details**, because `used_backup_quota`
hangs off them (1.10). Modrinth has five separate events for this (`backup.new`, `backup.patch`,
`backup.delete`, `backup.operation.*.init/start/done`, `types.ts:924-952`) but treats them all the
same: `server-panel-sync.ts:83-86` routes every `backup.*` into the same function, which
invalidates both queries (`:220-223`). Five messages for one effect are four too many.

Both messages go to **all** sessions with `BASE_READ` on this server, not only to the one that
triggered them. Without that, a second editor sees a list that is no longer right, and their next
click ends in a `404`.

---

## 4. Data types

```ts
// ---- What goes on the wire -----------------------------------------------

export type BackupStatus = 'pending' | 'in_progress' | 'timed_out' | 'error' | 'done'

export type BackupOperationType = 'create' | 'restore'

export type BackupOperationState =
  | 'pending'
  | 'ongoing'
  | 'completed'
  | 'cancelled'
  | 'failed'
  | 'timed_out'

/** Identical to Archon.BackupsQueue.v1.UserInfo. */
export interface BackupUserInfo {
  id: string
  username: string
  avatar_url: string | null
}

/** Identical to Archon.BackupsQueue.v1.BackupQueueOperation, except for operation_id. */
export interface BackupOperation {
  operation_type: BackupOperationType
  /** ULID. Passed through in the adapter as `unknown as number`, see A4. */
  operation_id: string
  state: BackupOperationState
  scheduled_for: string
  started_at: string | null
  completed_at: string | null
  has_parent: boolean
  error: string | null
  should_prompt: boolean
  /** always false; exists only because the type demands it. */
  synthetic_legacy: false
  user_info: BackupUserInfo | null
}

/** Identical to Archon.BackupsQueue.v1.ActiveOperation, except for operation_id. */
export interface BackupActiveOperation {
  backup_id: string
  operation_type: BackupOperationType
  operation_id: string
  has_parent: boolean
  scheduled_for: string
  started_at: string | null
  synthetic_legacy: false
  user_info: BackupUserInfo | null
}

/** Identical to Archon.BackupsQueue.v1.BackupQueueBackup plus size_bytes. */
export interface Backup {
  id: string
  name: string
  created_at: string
  /** State of the most recent operation of any kind — a restore included. See 1.2. */
  status: BackupStatus
  /** always false; we know no locked backups. */
  locked: false
  automated: boolean
  /** 0 while status != 'done'. Not part of Modrinth's type. */
  size_bytes: number
  /** newest first, at most 20. */
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
  /** Name of the safety backup that is created beforehand. */
  name: string
}

export interface RestoreBackupResponse {
  restore_operation_id: string
  safety_backup: {
    id: string
    create_operation_id: string
  }
}

export interface RetryBackupResponse {
  operation_id: string
  operation_type: BackupOperationType
}

export interface BulkDeleteBackupsRequest {
  backup_ids: string[]
}

export interface BulkDeleteBackupsResponse {
  deleted: string[]
  failed: Array<{ id: string; error: string; message: string }>
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
  next_run_at: string | null
  last_run_at: string | null
  last_status: BackupScheduleStatus | null
  last_error: string | null
}

export type UpdateBackupScheduleRequest = Pick<
  BackupSchedule,
  'enabled' | 'interval_hours' | 'hour_utc' | 'keep_last'
>

export interface ApiError {
  error: string
  message: string
}

// ---- WebSocket ------------------------------------------------------------

export type BackupProgressState = 'pending' | 'ongoing' | 'done' | 'failed' | 'cancelled'

export interface BackupProgressMessage {
  type: 'backup_progress'
  backup_id: string
  operation_id: string
  operation: BackupOperationType
  state: BackupProgressState
  /** 0.0–1.0. */
  progress: number
}

export interface BackupListChangedMessage {
  type: 'backup_list_changed'
}

export type BackupSocketMessage = BackupProgressMessage | BackupListChangedMessage

// ---- Error codes of this area --------------------------------------------

export type BackupErrorCode =
  | 'unauthorized'
  | 'forbidden'
  | 'not_found'
  | 'invalid_name'
  | 'invalid_schedule'
  | 'rate_limited'
  | 'backup_operation_in_progress'
  | 'backup_limit_reached'
  | 'backup_not_restorable'
  | 'backup_not_downloadable'
  | 'server_running'
  | 'server_installing'
  | 'nothing_to_retry'
  | 'not_cancellable'
  | 'not_acknowledgeable'
  | 'disk_full'
```

---

## 5. Open questions and assumptions

### A1 — We have automatic backups. *(assumption)*

The plan is silent. In favor:

- The interface distinguishes them visibly. Filter pills "Manual" / "Auto"
  (`backups.vue:359-362`), an icon of its own, `ShieldIcon` against `UserRoundIcon`
  (`BackupItem.vue:84-89`), the badge "Auto" (`:219-223`), an own fallback text "Backup schedule"
  instead of "Manual backup" (`:256`). Without a schedule those are four dead branches and a filter
  pill that never finds anything.
- We have to deliver the field `automated` anyway.
- The effort is one timer plus the same create function.

Scope deliberately small: **an interval in hours plus a time of day, no cron**, plus `keep_last`.
The default is **off**; whoever switches it on chooses disk usage on purpose. The cleanup concerns
**only** automatic backups — a schedule rule must never delete a backup made by hand. If the server
is not running, the backup still happens (and is even more consistent then). If nothing has changed
since the last automatic backup, it is skipped: `last_status: "skipped_unchanged"`, no new row, no
disk usage.

The comparison runs against the **completion time** of the last automatic backup (the `completed_at`
of its `create` operation), not against `created_at`. `created_at` is the moment of queuing (1.2);
every file the server touches while the packing runs would be newer than that, and the check would
never come out in favor of "unchanged". What is compared is the newest `mtime` **inside the set that
gets backed up**, so the exclusion list from A2 drops out with it, otherwise `logs/latest.log`
alone would keep every server permanently "changed".

If the decision goes the other way, everything else stands: `automated` is then a constant `false`,
2.12/2.13 fall away, and there are two endpoints fewer.

### A2 — What is backed up is the whole server directory, not only the world. *(decision, differs from the plan)*

The plan says "world" (`docs/PLAN.md:452`). That is not enough:

- Modrinth's own text describes more: "Saving world data **and server configuration**"
  (`BackupAdmonition.vue:118-121`) and "will replace the current world **and server files**"
  (`BackupRestoreModal.vue:8-9`).
- The "Reinstall" page promises: a reset deletes worlds, mods and configuration, "**Backups will
  remain and can be restored**" (`ui/src/layouts/shared/server-settings/pages/installation.vue:129`).
  A world-only backup would leave, after the reset, a world without the mods that produced it — a
  world that does not load.
- `InlineBackupCreator` is offered before **mod deletions**, bulk updates, modpack changes and ZIP
  uploads (`ConfirmDeletionModal.vue:17`, `FileUploadZipUrlModal.vue:57`). Exactly those operations
  touch `mods/`, `config/` and files, not the world. A world backup protects against none of them.
- Velocity has no world (`docs/PLAN.md:396,404`). "The whole directory" carries there without a
  special case; "the world" would need one.

Excluded are: `logs/`, `crash-reports/`, `cache/`, `*.log.gz`, Unix sockets and everything that is a
reference to outside the server directory (symlinks are not followed and not copied along).
`libraries/` and the loader jars stay **in** — without them a Forge or NeoForge server does not
start after a restore.

Format: one `tar`, zstd-compressed, level 3. No incremental backups, no deduplication: every backup
is a self-contained state that you can also unpack by hand with `tar`. The price is disk space, and
`max_backups` and `keep_last` cap it.

### A3 — Create without stopping, restore only at a standstill. *(decision)*

What Modrinth does can be read off the interface: `backupCreationDisabled`
(`backups.vue:618-636`) checks the permission, the quota, the busy reasons and running operations —
**not** `isServerRunning`. `backupRestoreDisabled` (`:602-616`) does check it, and the modal spells
it out: "Stop the server before restoring a backup" (`BackupRestoreModal.vue:4-6`). So: creating
while running is allowed, restoring is not.

**Whether Modrinth sends `save-off`/`save-all` while doing it is in no line of the code available
here** (a search for `save-off`, `save-all`, `save-on` in `vendor/modrinth/` and
`/root/ref-modrinth`: zero hits). The server-side backup tool is not part of the open code. I claim
nothing about it.

Our way when creating, if the server is running and is not a proxy:

1. `save-off` to standard input.
2. `save-all flush`.
3. Wait for the console line `Saved the game` (Vanilla/Paper from 1.13 on) or `Saved the world`
   (older), at most 30 s.
4. Read the directory and pack it.
5. `save-on` — **always**, on a cancel, an error or a timeout too; otherwise the server's automatic
   saving stays off for good. That is the only way in which this feature can do real damage, and it
   should be secured accordingly (a `Drop` guard in Rust, not an `if` at the end).

If the server does not answer within 30 s, the packing happens anyway and a warning is written to
the console. There is no field of its own for it: the interface would have no place to show it
(`error` is only shown for `failed`/`timed_out`, `BackupAdmonition.vue:97-99`), and an invisible
field is a lie.

Velocity and other proxies: steps 1–3 and 5 fall away with no replacement (no world, no `save`
commands).

Restoring: the server has to be stopped, and that is checked on the server side
(`409 server_running`). The flow: safety backup → fill a new directory
`<server-dir>.restoring-<operation-id>` → rename the old directory to `<server-dir>.old-<operation-id>`
→ rename the new one into its place → delete `.old-…`. If it breaks off before the last step, the
rename is undone; the server is never half restored. The operation id in both names is the
countermeasure against a leftover `.old`: a fixed name that could not be deleted once would have
ended every further restore of this server with `ENOTEMPTY` for good. A start during a running
restore is refused — the "server control" area has to check that too, see F1.

**Who owns the newly unpacked directory?** Here the flow runs into the permission scheme from P0,
and hard. The panel runs as `craftpanel` (`docs/PLAN.md:179`), the server process as `craft-<id>`,
and the group `craftpanel` contains **only the panel service** (`docs/PLAN.md:160-161`). The
existing server directory carries `craft-<id>:craftpanel 2770` (`:155`): the server gets at it as
the owner, the panel through the group. If the panel now unpacks a new tree, every file in it
belongs to `craftpanel`, and `craft-<id>` is neither its owner nor a member of its group. Nothing
is set for "other". **After the restore the server could no longer read its own files.** A `chown`
to a foreign user needs root, and the helper's vocabulary knows exactly three commands, and `chown`
is not one of them (`docs/PLAN.md:187-189`).

That is not a small matter of this area but a gap in P0 — it hits every write of the panel into a
server directory, only most visibly here, because a restore rewrites *every* file. See F7. Until
that is decided, this section assumes a fourth helper command `chown-tree <uid> <path>` that runs
after the unpacking and before the rename and, like `spawn`, only accepts uids the helper created
itself.

### A4 — `operation_id` is a ULID that the adapter hands out as a `number`. *(assumption, touches a cross-area ruling)*

`BackupQueueOperation.operation_id` is `number | null` (`types.ts:874,884`), and the client methods
take `operationId: number` (`backups-queue/v1.ts:33,41,49,57`). Our ruling says: ids are ULIDs, never
running numbers. Both at once is impossible.

Checked what the adopted files actually **do** with the value: check it for `!= null`
(`backups.vue:424-427`, `use-inline-backup.ts:109,152-160`), compare it for equality
(`ServerPanelAdmonitions.vue:121-122`), put it into a key (`:115,141`) and hand it back to the client
(`:295-344`). **Nowhere is it computed with, sorted, or compared with `<`.** Strings work at all five
places at runtime.

So: a ULID on the wire, and in the adapter exactly one documented line
`operation_id: op.operation_id as unknown as number`. The alternative — a `u32` running per server —
would break the global rule for a field that never appears in a URL. The decision belongs to whoever
owns the global rulings; here it is named, not hidden.

### A5 — Operation records yes, a queue no. *(decision)*

Modrinth's model separates the backup from the operation and has `ack` and `cancel`. Of that we
need:

- **Operation records with a history**: mandatory. `history` is part of the type, `history[0]` drives
  the banner (`ServerPanelAdmonitions.vue:137-156`), and the author is in it
  (`BackupItem.vue:71-76`).
- **`ack`**: mandatory, otherwise every success or error banner that was clicked away comes back on
  the next load. `should_prompt` is exactly that flag (`:139`).
- **`cancel`**: mandatory, it is called from two adopted files
  (`ServerPanelAdmonitions.vue:315-340`, `use-inline-backup.ts:147-176`). A backup of a 6 GB modpack
  takes minutes; without a cancel the interface is locked for the duration (`busyReasons`,
  `server-backups-queue.ts:92-110`).
- **A real queue with a depth > 1**: not needed. The interface itself prevents a second operation
  from being queued (`hasActiveCreate` / `hasActiveRestore` lock the buttons,
  `backups.vue:612-614,632-634`). We enforce the same with `409 backup_operation_in_progress`.
  `pending` stays as a state — between the acceptance of the request and the start of the copying
  lie fractions of a second, and the type demands the state anyway.

That drops Modrinth's `scheduled_for` waiting time as a real concept; we carry the field on and set
it to the moment of acceptance.

`should_prompt` precisely: `true` for every terminal state of an operation triggered by hand, until
the `ack` comes. `false` for successful **automatic** backups. Otherwise the panel greets you every
morning with a success banner. For **failed** automatic backups `true`: you have to find out that
the schedule is not running.

Timeout: no progress for over 10 minutes → operation to `timed_out`, backup to `timed_out`, send
`save-on`, delete the partial file. That gives `timed_out` from `types.ts:851` a defined trigger
instead of merely existing.

### A6 — Backups live outside the server directory. *(decision, reaches into the directory layout from P0 → F2)*

```
/var/lib/<panel>/
├── backups/
│   └── <server-id>/
│       └── <backup-id>.tar.zst        craftpanel:craftpanel  0600
└── users/
    └── <user-id>/servers/<server-id>/    craft-<id>:craftpanel  2770
```

Three reasons why they must **not** go under the server directory:

1. A backup of the whole directory (A2) would otherwise pack all older backups along with it.
2. The file manager shows everything below the server directory. An editor could delete backups
   without holding the `BACKUPS` permission — the roles could be bypassed.
3. A reset deletes the server directory; "Backups will remain" (`installation.vue:129`) would be
   broken.

The owner is **`craftpanel`, not `craft-<id>`**, with `0700`. That is stricter than the layout in
`docs/PLAN.md:150-158`, and on purpose: the server process runs as `craft-<id>` and would get at
everything that belongs to that user. A plugin that has gone off the rails should not be able to
delete the backups — that is the one place where this separation really pays off. The panel reads
the server directory through the group `craftpanel` (`docs/PLAN.md:166`) and writes the backup as
itself; **for creating** it therefore needs neither root nor the helper. For **restoring** that does
not hold — there the panel writes back into the server directory and the ownership question flips;
see the box at the end of A3 and F7.

So the backups do **not** count into the server's `storage_usage_bytes`
(`ui/src/providers/server-context.ts:24`) — that value measures the server directory. The metrics
area has to know this (F4).

### A7 — Quota: a count per server, byte budget open. *(decision + gap)*

`Server.backup_quota` and `used_backup_quota` (`types.ts:610-611`) are **counts** at Modrinth, and
`backups.vue:621-628` compares a count against a count. We adopt that:

- `max_backups` per server, default **10**, changeable by the administrator, **at most 50** (2.1).
- Reached → `409 backup_limit_reached`; automatic backups clean up by `keep_last` beforehand and
  otherwise report `last_status: "skipped_limit"`.
- `backup_quota` in the server object carries exactly this value, `used_backup_quota` the count of
  all backup rows including the ones still running — `backups.vue:624` counts
  `backups.value.length`, not only the finished ones. Counting only `done` in the backend gives a
  display that does not match the refusal.

One consequence you have to know: safety backups from 2.6 are `automated: false` and are **not**
cleaned up by `keep_last`. Whoever restores often fills their quota with them, and then the next
restore fails on `backup_limit_reached` through no fault of its own. The way out is deleting by
hand. That is the price for a cleanup rule never touching a non-automatic backup, and the rule
matters more to me.

There is no **byte budget per user**, because the plan has none: the limits per user are
`memory.max`, `memory.high`, `cpu.max`, `pids.max` (`docs/PLAN.md:229-235`) — cgroups limit no disk
space. Until that is decided (F3), only the brake against a full disk applies: before every create
the free space has to be ≥ estimated size × 1.1, otherwise `507 disk_full`. We deliver `size_bytes`
from the start, so that a later budget needs no data migration.

### A8 — Download through a compatibility path. *(decision)*

`BackupItem.vue:112-118` builds the URL itself and disables the menu item when `kyrosUrl` or `jwt`
is missing. Three ways: (a) serve the path `/modrinth/v0/backups/{backup_id}/download` ourselves,
(b) leave the menu item permanently gray, (c) change the component. (c) breaks "adopt unchanged",
(b) throws away a useful function and looks like a bug. So (a) — an alias onto 2.10, with cookie
authorization instead of the token in the URL. Over HTTP instead of HTTPS only (b) remains, see L2.

At Modrinth the two props come from `server.node` (`types.ts:654-657`, passed on in
`backups.vue:163-164`) — the node concept that `docs/PLAN.md:93` rules out. So we do **not** fill
`node` but set the props directly in our rebuilt page. The only other reader of `server.node` is a
"Node" field in `server-settings/pages/general.vue:326`, which shows "Unknown" without a value.

### F1 — Who prevents a start during a restore? *(somebody else has to decide)*

We deliver `busyReasons` through the operation state (`server-backups-queue.ts:92-110` produces them
from `hasRunningCreate`/`hasRunningRestore`) — provided two conditions are met: `status` flips to
`in_progress` for a restore as well (1.2), and the server frame passes the reasons into the context
as `extraBusyReasons` (1.10). Then the interface locks. On the server side the "server control" area
has to refuse `POST .../start` as long as a `restore` is running. Proposed code:
`409 backup_restore_in_progress`.

### F2 — Directory layout and permissions. *(area P0 has to confirm)*

A6 creates `/var/lib/<panel>/backups/` and claims `craftpanel:craftpanel 0700`. That is not in
`docs/PLAN.md:150-158`. If something else is decided there, only a path changes here, but the
permission assignment is a security statement and should be confirmed.

### F3 — Byte budget per user. *(the accounts/limits area has to decide)*

See A7. If it comes: a check before 2.2 and 2.6, error code `storage_limit_reached` (409), and the
safety backup from 2.6 counts too.

### F4 — Do backups count into the displayed disk usage? *(the metrics area has to decide)*

Our proposal: no, `storage_usage_bytes` stays the server directory. Otherwise the value jumps with
every backup.

### F5 — `/user/:username`. *(the accounts area has to decide)*

See L1. Either the route exists, or the author name in every backup row is a dead link.

### F6 — Retention when a server or a user is deleted. *(the accounts area has to decide)*

`docs/PLAN.md:344-346` demands an explicit decision about a user's servers when the user is deleted.
The same question applies to their backups, and it is more uncomfortable: backups are exactly what
you still want after a mishap. Proposal: on a transfer they move along; on a deletion they are
deleted along with it, but the confirmation names the count and the total size.

### F7 — Who owns a restored server directory? *(area P0 has to decide)*

Spelled out at the end of A3. In short: the panel runs as `craftpanel`, the server as `craft-<id>`,
and `craft-<id>` is not in the group `craftpanel` (`docs/PLAN.md:160-161`). Everything the panel
creates anew belongs to `craftpanel` and is out of reach for `craft-<id>`. On a restore that
concerns every file of the server.

Three ways out, and none of them is mine:

1. **A fourth helper command `chown-tree <uid> <path>`** that works only below
   `/var/lib/<panel>/users/` and only on uids it created itself. That is my proposal — it fits the
   cut of the three existing commands (`docs/PLAN.md:187-194`) and solves the problem for all areas
   at once.
2. **Put `craft-<id>` into a second group per user** and share the files through it. Fails on the
   same reasoning with which the plan chose a single group: group membership is fixed when the
   process starts (`docs/PLAN.md:171-173`).
3. **Run the unpacking as `craft-<id>`**, through `spawn`. Then the helper has nothing new to learn,
   but a server user would get read access to the backup file, and that is exactly what A6 takes
   away from it on purpose.

As long as that is open, the flow in A3 applies with way 1. If the decision goes the other way,
nothing changes about the endpoints of this document.


---

## 4. Backups in the user's Google Drive

Added 2026-08-14. The area is section **22** of the contract; the draft with the measurements
against Google is in `docs/DRIVE.md`. Here only what changes about *this* document.

**The endpoints from 2 stay as they are.** Two are added —
`GET`/`PUT /api/v1/servers/{server_id}/backups/target` (22.9, 22.10) — and three responses grow by
fields or errors:

| Place | Addition |
|---|---|
| `BackupQueueBackup` (1.2) | `location`, `drive_state`, `drive_verified`, `drive_content_changed`, `drive_web_link` — in **every** row; on a local one the last four are `null`. `drive_verified: false` means Google named no checksum for that archive, so nothing confirms it arrived whole; `drive_content_changed: true` means the file is still in the Drive but no longer holds that archive |
| Create (2.2) | `409 drive_not_connected` and `409 drive_not_configured` as soon as the target is `drive`; **both disk questions still stand**, because the archive is built here first in any case |
| Restore (2.6) | `409 backup_not_restorable` for `drive_state ∈ {missing, trashed}` and for `drive_content_changed: true` as well |
| Download (2.8, 2.11) | `409 backup_lives_in_drive`; the panel transfers not one byte, the way is `drive_web_link` |

**Modrinth's component stays unchanged.** `BackupItem` does not know the five fields; they simply
ride along through `toQueueBackup` (`web/src/composables/archon-adapters.ts`), and the backups page
reads them through `driveFactsOf` (`web/src/pages/servers/backup-target.ts`) — one place where the
look-up happens exactly once, instead of writing an `as` at every use. The button "Open in Drive"
sits in the same place where the fallback download already hung (2.11). Beside the state badge the
page carries one more sentence: for a row with `drive_state: "present"` and `drive_verified: false`
it says out loud that nobody confirmed this one, because a backup whose soundness was never checked
is a hope and must not read like a promise. Beside it stands the harder case: for
`drive_content_changed: true` the green badge gives way to a red "Not this backup any more" and a
sentence saying that the file in the Drive has been written over since — a row that still reads
"In Google Drive" while holding somebody else's bytes is the one lie this page must not tell.

**Two rules from this document stay untouched on purpose:**

* The **item quota** (A7) counts Drive backups too. If they did not count, "50 local and 50 in
  Drive" would be a way around the limit.
* The **disk limit** (F3, by now 12.7) does **not** count them: `auth/disk.rs` now only sums
  `WHERE … AND b.location = 'local'`. `size_bytes` stays set — 2.1 shows the size — but a backup of
  which not one byte lies here must not weigh on the account's pot forever.

**The storage location from A6 stays the build site.** A backup "into the cloud" also comes into
being as a file in `<data_dir>/backups/<server-id>/` and is only deleted there after the upload. The
reason is in `docs/DRIVE.md` §4 and is the same as in A3: `save-off` may only be in force for as
long as the packing runs, and not for as long as the upload runs.
