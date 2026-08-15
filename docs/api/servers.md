# Interface: servers — inventory, creating, power, state

As of 2026-08-12. Source references are paths relative to `/root/MinecraftServerManager/`.
Everything under `vendor/modrinth/` is foreign, unchanged code; every claim about what the
interface needs stands there with a file and a line.

**A note on two paths.** While this document was being written, the debranding pass
(task "P0: debranding") deleted `vendor/modrinth/ui/src/layouts/wrapped/`
and `vendor/modrinth/ui/src/components/servers/ServerListing.vue` on 2026-08-12 at about 17:10.
The evidence from them can be read line by line in the reference clone:
`/root/ref-modrinth/packages/ui/src/<the same path>` — checked, the line numbers agree there.
All remaining citations (`layouts/shared/`, `components/servers/` without `ServerListing.vue`,
`composables/`, `providers/`, `vendor/modrinth/api-client/`) still point into `vendor/`.

The line numbers in `docs/PLAN.md` refer to its state of 2026-08-12, 17:15
(505 lines); the plan is being edited in parallel right now.

The scope of this document: listing, creating, deleting and renaming servers, power
(start/stop/restart/kill), the live state over WebSocket, the metrics and the loader catalog
for the creation wizard. Not here: files, console, content, backups, settings,
access/roles. Where the areas touch, it says so explicitly.

---

## 1. The provider contract

### 1.1 `ModrinthServerContext` — 22 members, one by one

Declaration: `vendor/modrinth/ui/src/providers/server-context.ts:37-72`.
Modrinth's own reference filling: `vendor/modrinth/ui/src/composables/server-manage-core-runtime.ts:390-415`
— we read it as a model, because it shows which fields come from HTTP and which the provider
computes itself.

| # | Field | Type (source) | Where it comes from here |
|---|---|---|---|
| 1 | `serverId` | `string`, `server-context.ts:38` | route parameter; identical to `server.server_id` from `GET /api/v1/servers/:id` |
| 2 | `worldId` | `Ref<string \| null>`, `:39` | **the constant `"default"`** in the provider. We have one world per server; our paths carry no world segment. Reasoning and consequences in 5.2 |
| 3 | `server` | `Ref<Archon.Servers.v0.Server>`, `:40` | `GET /api/v1/servers/:id`, then live over the WS `server` message. Field-by-field mapping in 1.2 |
| 4 | `serverFull` | `ComputedRef<ServerFull \| null>`, `:41` | **constantly `null`.** `ServerFull` (`vendor/modrinth/api-client/src/modules/archon/types.ts:716-726`) has nine fields; we would have `id`, `name` and `tags`, but not `subdomain`, `specs`, `sftp_username`, `sftp_password`, `location`, `worlds`. Checked: not a single reader in `layouts/shared/` or `components/servers/`; the one occurrence, `composables/server-backup.ts:14`, pulls its list from a query of its own, not from the context |
| 5 | `currentUserPermissions` | `ComputedRef<UserScope>`, `:42` | `server.current_user_permissions`. We deliver **names instead of a number** — reasoning in 1.3 |
| 6 | `isConnected` | `Ref<boolean>`, `:45` | provider: `true` on the WebSocket's `open`, `false` on its `close`. No API field |
| 7 | `isWsAuthIncorrect` | `Ref<boolean>`, `:46` | provider: `true` on WS close code `4401`/`4403` (3.4). The overlay message "Could not connect to the server" hangs on it: `layouts/wrapped/hosting/manage/overview.vue:28-36` |
| 8 | `powerState` | `Ref<PowerState>`, `:47` | WS `state.power_state`. The value range is identical to Modrinth's: `running \| stopped \| starting \| stopping \| crashed` (`types.ts:1067`) |
| 9 | `powerStateDetails` | `Ref<{oom_killed?, exit_code?} \| undefined>`, `:48` | WS `state.oom_killed` / `state.exit_code`. Only set when `power_state === "crashed"`, otherwise explicitly `undefined` — that is what the reference does too (`server-manage-core-runtime.ts:205`). Today the field has no reader in `layouts/shared/` or `components/servers/` |
| 10 | `isServerRunning` | `ComputedRef<boolean>`, `:49` | provider: `powerState === 'running'`. Readers: `components/servers/backups/BackupRestoreModal.vue:4,93` |
| 11 | `stats` | `Ref<ServerStats>`, `:50` | WS `stats`, worked up in the provider into `{current, past, graph}`. Details in 1.4 |
| 12 | `uptimeSeconds` | `Ref<number>`, `:51` | WS `state.uptime_seconds`; between two messages the provider counts up locally once a second (`server-manage-core-runtime.ts:137-142`) and sets it to 0 on `stopped`/`crashed` (`:206-209`) |
| 13 | `isSyncingContent` | `Ref<boolean>`, `:54` | **content area** (modpack sync, pending addon installations), binding: `docs/api/content.md`. **Nothing** comes to it from my area: `state.install != null` already lands in `busyReasons` through `status === 'installing'`; a second time would give two reasons for the same operation and a wrong tooltip (`use-server-power-action.ts:49` takes the first) |
| 14 | `busyReasons` | `ComputedRef<BusyReason[]>`, `:57` | **No API field.** The provider computes it from `server.status === 'installing'` and `isSyncingContent`, exactly as `server-manage-core-runtime.ts:108-128` does. In detail in 1.5 |
| 15 | `fsAuth` | `Ref<FilesystemAuth \| null>`, `:60` | **files area.** We have no second node and no JWT; the provider sets `{ url: "", token: "" }` or `null`. `docs/api/files.md` is binding |
| 16 | `fsOps` | `Ref<FilesystemOperation[]>`, `:61` | files area (WS message `fs_ops`) |
| 17 | `fsQueuedOps` | `Ref<QueuedFilesystemOp[]>`, `:62` | files area |
| 18 | `refreshFsAuth` | `() => Promise<void>`, `:63` | files area; here probably an empty function |
| 19 | `uploadState` | `Ref<UploadState>`, `:66` | files area |
| 20 | `cancelUpload` | `Ref<CancelUploadHandler \| null>`, `:67` | files area |
| 21 | `activeOperations` | `ComputedRef<FileOperation[]>`, `:70` | files area |
| 22 | `dismissOperation` | `(opId, action) => Promise<void>`, `:71` | files area |

**The count.** The contract has 22 members. This document fills thirteen of them (1–12, 14)
completely — each with a value, even where the value is a constant. The nine remaining ones belong
to other areas: one (13) to the content area, eight (15–22) to the files area. They stand here only
so that the list is complete; I do not define their values.

`BusyReason` is **not** a message object but `{ reason: MessageDescriptor }`
(`server-context.ts:9-11`). Whoever builds it has to use `defineMessage({ id, defaultMessage })`:
the interface reads `r.reason.id` (`use-server-power-action.ts:27`) and passes `r.reason` on to
`formatMessage` (`use-server-power-action.ts:49`). A flat `{ id }` would mean an access on
`undefined` there.

### 1.2 The `server` object: `Archon.Servers.v0.Server`, 23 fields

Declaration: `vendor/modrinth/api-client/src/modules/archon/types.ts:604-629`.
Our `GET /api/v1/servers/:id` does **not** deliver this object but our own type
`Server` (section 4). The adapter in the frontend (`web/src/api/server-adapter.ts`, still to be written)
assembles the vendor object out of it. The third column is the adapter.

| Field | Who reads it | Value here |
|---|---|---|
| `server_id` | everywhere | `Server.server_id` (ULID) |
| `name` | `ServerListing.vue:40`, `ServerSettingsModal.vue:228`, `server-settings/pages/general.vue:174` | `Server.name` |
| `owner_id` | own/shared split: `layouts/wrapped/hosting/manage/index.vue:566-568` | `Server.owner_id` |
| `net` | `labels/ServerInfoLabels.vue:24-25` (`domain` only), `server-settings/pages/network.vue:248-249` (`ip`, `port`), **`server-settings/pages/general.vue:175,344-347,357,365-381`** (`domain` as an input field with a check and a write path) | `Server.net`. **`domain` is always `""`** — with a consequence that blocks renaming: 5.3 |
| `game` | `labels/ServerGameLabel.vue:13,23` | the constant `"Minecraft"` |
| `backup_quota` | no reader in `layouts/shared/` or `components/servers/` | the adapter sets `0`; the backups area may override it |
| `used_backup_quota` | no reader | the adapter sets `0` |
| `status` | `use-server-power-action.ts:23`, `ServerListing.vue:451,463,477,560`, `admonitions/ServerPanelAdmonitions.vue:63`, `server-settings/pages/installation.vue:205` | `Server.status`, value range `installing \| available \| broken` |
| `suspension_reason` | `ServerListing.vue:478-489` (only when `status === 'suspended'`) | constantly `null`; we know no `suspended` |
| `loader` | `ServerSetupModal.vue:98`, `installation.vue:425`, `labels/ServerLoaderLabel.vue:5,15` | `Server.loader`, in display spelling (`"Paper"`, `"NeoForge"`, `"Folia"`). `Archon.Servers.v0.Loader` (`types.ts:590-598`) knows only eight values and neither `Folia` nor `Leaf` nor `Velocity` — the adapter casts. Everything is displayed all the same: `ServerLoaderLabel.vue:15` prints the string raw, and `TagIcon` renders nothing only when there is no icon (`TagIcon.vue:20`). Of our ten, exactly `leaf` is missing from `assets/generated-icons.ts:979-1009` |
| `loader_version` | `installation.vue:432`, `ServerLoaderLabel.vue:16` | `Server.loader_version` |
| `mc_version` | `ServerSetupModal.vue:103`, `installation.vue:429`, `ServerGameLabel.vue:14,24` | `Server.mc_version` |
| `upstream` | `ServerListing.vue:525-531` (loads the modpack title from api.modrinth.com) | `Server.upstream`; the content area maintains it, `null` on a loader server |
| `sftp_username` | `server-settings/pages/advanced.vue:47-50` | **deliberately missing** (PLAN.md:97). The adapter sets `""`; the "Advanced" page is gutted by the settings area |
| `sftp_password` | `advanced.vue:64-70` | `""`, ditto |
| `sftp_host` | `advanced.vue:31-34` | `""`, ditto |
| `datacenter` | no reader | `""` |
| `notices` | no reader in our scope | `[]` |
| `node` | `general.vue:326` shows `node.instance` as "Node" | `null` → the line shows "Unknown". The settings area decides whether the line stays |
| `flows` | `ServerListing.vue:447` ("New" chip), `index.vue:549` (sorting) | the constant `{ intro: false }`. Here a server is fully configured after `POST /servers` — there is no follow-up setup step as at Modrinth (`onboarding.vue`), so never an onboarding state |
| `is_medal` | `index.vue:175,181` (card variant), `general.vue:76` | the constant `false` |
| `current_user_permissions` | `composables/server-permissions.ts:85-99` | `Server.current_user_permissions`, see 1.3 |
| `medal_expires` | `index.vue:480` | leave out |

Eleven of the twenty-three fields exist only because of Modrinth's business model and are filled
by the adapter with constants. Not a single one of them makes it into our API.

### 1.3 `current_user_permissions`: names instead of a number

`Archon.Servers.v0.UserScope` is declared as `number` (`types.ts:631`), but the bits sit right at
the top: `BASE_READ = 1n << 63n`, `POWER_ACTIONS = 1n << 62n`, …, `SUPPORT_AGENT = 1n`
(`vendor/modrinth/ui/src/composables/server-permissions.ts:15-32`). An IEEE 754 double carries 53
mantissa bits. As long as only the upper bits (55–63) are set, the number is exactly
representable — all three of our roles fall into that range. As soon as somebody mixes an upper
and a lower bit, JSON quietly loses precision.

Therefore: **we send an array of names.** At runtime `parsePermissions` also takes strings
and splits them at `|` (`server-permissions.ts:39-53`); `ServerUsers.v1.UserScope` is
explicitly `string | number` (`types.ts:531`). The adapter sets
`current_user_permissions: perms.join('|') as unknown as number` — one cast in one place,
and in exchange no rounding problem and a readable protocol.

The mapping role → bit list does **not** belong here but in `docs/api/auth.md`, 1.3
(PLAN.md:479-480: three roles over ten permission bits).

### 1.4 `stats`: what the interface really shows

`ServerStatsSample` has five fields (`server-context.ts:20-26`). What `ServerManageStats.vue`
makes of them:

| Field | Use | Source |
|---|---|---|
| `cpu_percent` | "CPU" tile, `toFixed(2)`, warning color from 90 up | `ServerManageStats.vue:225,138` |
| `ram_usage_bytes` | "Memory" tile, as a percentage of `ram_total_bytes` or as bytes | `:135,236` |
| `ram_total_bytes` | denominator of the percentage **and** of the graph | `:135`, `server-manage-core-runtime.ts:150` |
| `storage_usage_bytes` | "Storage" tile | `:187` |
| `storage_total_bytes` | **no reader.** Only `createInitialStats()` writes it | `server-manage-core-runtime.ts:44,51` |

`ServerStats.past` is set (`server-manage-core-runtime.ts:154`) but read by no component
— the provider carries it anyway, because the type demands it.

**History data.** `graph.cpu` and `graph.ram` are pure client buffers: `appendGraphData` appends
and, from the eleventh value on, drops from the front (`server-manage-core-runtime.ts:59-63`),
`ServerManageStats.padGraph` cuts to `GRAPH_SIZE = 10` and pads missing points at the front with
zeros (`ServerManageStats.vue:122-128`). `graph.ram` is not in bytes but
`floor(ram_usage / ram_total * 100)` (`server-manage-core-runtime.ts:148-151`), and `padGraph`
caps every value at 100 (`:125`).

Two things follow: the API transmits **no** history, and the visible period is
`10 × the metrics interval`. At our interval of 1 s that is the last ten seconds. So that the
graph does not start out empty after a page change, on connect the server sends the up to ten
buffered samples as ten ordinary `stats` messages, oldest first (3.2).

**The watchdog.** If `stats` stay away for longer than 5 s, the provider pushes zeros into
the graph on its own, once a second (`server-manage-core-runtime.ts:65-66,191-197`). While it
does, `storage_usage_bytes` stays put and only CPU and memory go to zero (`:177-183`). Our
interval therefore has to be below 5 s, and with a stopped server we may simply stop sending — the
tiles fall to 0 by themselves and the storage display stays correct.

### 1.5 `busy_reasons`: not a field but a computation

`busyReasons` comes into being in the client and nowhere else
(`server-manage-core-runtime.ts:108-128`). Every entry is a `{ reason: MessageDescriptor }`, not a
flat object (`server-context.ts:9-11`):

* `server.status === 'installing'` → `{ reason: defineMessage({ id: 'servers.busy.installing', … }) }`
* `isSyncingContent` → `{ reason: defineMessage({ id: 'servers.busy.syncing-content', … }) }`
* `extraBusyReasons` — the backups area appends `servers.busy.backup-creating` and
  `servers.busy.backup-restoring` (`composables/server-backups-queue.ts:92-110`)

The IDs are not just labels: `ServerPanelAdmonitions.vue:71-85` and `files.vue:61-66`
filter **by them**, so that the same reason does not appear twice as a banner. Whoever invents
reasons of their own has to hit these four strings.

Who uses it, and how:

| Place | Effect when not empty |
|---|---|
| `components/servers/server-header/use-server-power-action.ts:43` | **all** power buttons off, kill included |
| `use-server-power-action.ts:49` | tooltip = the first reason |
| `layouts/wrapped/hosting/manage/files.vue:53-62` | write access in the file manager locked |
| `layouts/wrapped/hosting/manage/content.vue:174-185` | content actions locked |
| `layouts/shared/server-settings/pages/general.vue:140`, `properties.vue:282` | save banner switched to "saving" |
| `layouts/shared/server-settings/pages/installation.vue:205,219` | reinstalling locked |
| `components/servers/admonitions/ServerPanelAdmonitions.vue:79-93` | the warning "Background task running" |

The API delivers exactly two inputs for this: `status` (REST and WS `server`) and `state.install`
(WS). There is no `busy_reasons` field and there should not be one — otherwise server and
client would both have to know the same rule.

One special case matters: `use-server-power-action.ts:21-30` treats "currently installing"
separately (button "Installing…", disabled) and checks both `server.status` and the
message IDs for it. As long as `status` is right, the display is right.

---

## 2. The endpoints

Common to all: session cookie, `Content-Type: application/json`, error body
`{ "error": "<code>", "message": "<text>" }`. The stable codes laid down here are carried by
`auth::error::Failure` itself. The `ApiError` enum with its catch-all codes (`bad_request`,
`conflict`), which was meant to be extended for this once, is never called and was
deleted on 2026-08-15 — CONTRACT.md 1.7.

The permission column: "session" = signed in; the uppercase names are server bits from
`server-permissions.ts:15-32`; "panel admin" is the panel role from PLAN.md:308.

### 2.1 `GET /api/v1/servers` — the inventory

Permission: session. Delivers the servers the signed-in user has `BASE_READ` on — their
own and the ones shared with them.

Query parameters:

| Name | Values | Meaning |
|---|---|---|
| `scope` | `visible` (default) \| `all` | `all` for panel admins only (PLAN.md:358), otherwise `403 forbidden` |

No pagination, no `query`. The reason: Modrinth searches in the client with Fuse over
`['name','loader','mc_version','game','state','owner.username']`
(`layouts/wrapped/hosting/manage/index.vue:592-604`, the key list in `:597`) and needs
the full list for that; on one machine there are dozens of servers, not thousands. (In passing: the
Fuse key `state` points at nothing — there is no field of that name on `Server`,
`types.ts:604-629`.)

The split "Your servers" / "Shared with you" is done by the list itself through
`server.owner_id === session.user_id` (`index.vue:566-567`, applied in `:580-590`); `users` is
there for that, delivering the display name of the other owner (`index.vue:570-578`).

Response `200`:

```json
{
  "servers": [
    {
      "server_id": "01J9ZC8QK7N3S1M4V6R2XP0TAB",
      "name": "Survival",
      "owner_id": "01J9Z0000000000000000MAX01",
      "status": "available",
      "game": "Minecraft",
      "loader": "Paper",
      "loader_version": "45",
      "mc_version": "1.21.8",
      "net": { "ip": "192.0.2.10", "port": 25565, "domain": "" },
      "ram_mib": 4096,
      "upstream": null,
      "current_user_permissions": ["SERVER_ADMIN"],
      "created_at": "2026-08-01T14:22:03Z"
    },
    {
      "server_id": "01J9ZC9RR4T5W8Y0Q1Z3B7C5DE",
      "name": "Friends modpack",
      "owner_id": "01J9Z0000000000000000KYM02",
      "status": "installing",
      "game": "Minecraft",
      "loader": "Fabric",
      "loader_version": "0.16.9",
      "mc_version": "1.21.1",
      "net": { "ip": "192.0.2.10", "port": 25566, "domain": "" },
      "ram_mib": 6144,
      "upstream": { "kind": "modpack", "project_id": "1KVo5zza", "version_id": "8xQZ4rTt" },
      "current_user_permissions": ["BASE_READ", "POWER_ACTIONS", "EXEC_COMMANDS", "FILES_WRITE"],
      "created_at": "2026-08-11T09:05:41Z"
    }
  ],
  "users": {
    "01J9Z0000000000000000MAX01": {
      "id": "01J9Z0000000000000000MAX01",
      "username": "max",
      "avatar_url": null
    },
    "01J9Z0000000000000000KYM02": {
      "id": "01J9Z0000000000000000KYM02",
      "username": "kim",
      "avatar_url": null
    }
  }
}
```

Errors: `401 unauthenticated`, `403 forbidden` (on `scope=all` without panel admin).

**Live behavior of the list.** There is no list-wide WebSocket (a ruling: one socket per
server). As long as a server in the list has `status === "installing"`, the page fetches the list
again every 5 s — Modrinth uses the same pattern while waiting for a freshly paid-for server
(`index.vue:505`, a `refetchInterval` that switches between `5000` and `false`, depending on
whether something is pending).

### 2.2 `POST /api/v1/servers` — creating

Permission: session. `owner_id`, a port outside the pool and a `ram_mib` above the
budget are reserved for panel admins (PLAN.md:348-358).

**Synchronous or an operation?** Both, parted at the right seam: the call **answers
synchronously with 201** as soon as the checks, the port assignment, the directory and the
database entry are in place and the download source is resolved; downloading and unpacking then
run on as an operation, and the server sits at `status: "installing"` while they do.

Three reasons why it cannot be done any other way:

1. The interface already has a display for exactly this state, and it hangs on the
   server WebSocket, not on an HTTP response: `InstallingBanner` is fed from
   `state.progress` (`layouts/wrapped/hosting/manage/root.vue:703-711`,
   `components/servers/InstallingBanner.vue:56-60,199-201`). A blocking POST could not serve this
   display at all.
2. `server.status === 'installing'` is the switch that locks every write path while it
   lasts (1.5). The switch only exists if the record exists first.
3. A Paper jar is 50 MB, a modpack quickly 500 MB. HTTP requests that run for minutes die on
   reverse proxy timeouts; the user would see nothing but a spinner without progress.

What happens **before** the 201, by contrast, is everything that may fail without leaving a
corpse behind: the name check, EULA, budget, port assignment, resolving loader and version against
the source (Mojang manifest, PaperMC v3 …, PLAN.md:373-392). A typo in the version thus becomes
a `400`, not a broken server.

**The budget check and the port assignment are in one transaction.** Both are check-then-act:
otherwise two concurrent requests from the same user read the same `-Xmx` sum and both get
through, and two concurrent requests from any users get the same free port. The
record is therefore created in **one** SQLite transaction, which reads and writes the sum of the
owner's `ram_mib` and the ports in use in the same move; the port column carries a
uniqueness constraint, so that the failure is still called `409 port_unavailable` when two
transactions overtake each other. Resolving against the source happens **before** the transaction,
because it is network traffic and must not hold a lock.

Request (`application/json`):

```json
{
  "name": "Survival",
  "source": {
    "kind": "loader",
    "loader": "paper",
    "loader_version": null,
    "mc_version": "1.21.8"
  },
  "ram_mib": 4096,
  "port": null,
  "owner_id": null,
  "accept_eula": true,
  "properties": {
    "known": {
      "gamemode": "survival",
      "hardcore": "false",
      "difficulty": "normal",
      "level_seed": null,
      "level_type": "minecraft:normal",
      "generate_structures": "true"
    }
  }
}
```

* `source.loader` is the lowercase variant from the wizard
  (`CreationFlowContextValue.selectedLoader`, `creation-flow-context.ts:157`).
* `source.loader_version: null` means "the newest stable build"; the server resolves it and writes
  the result into `Server.loader_version`.
* `properties` is the output of `ctx.buildProperties()` unchanged
  (`creation-flow-context.ts:524-542`, type `Archon.Content.v1.PropertiesFields`,
  `types.ts:419-422`). We pass it through and write `server.properties` from it
  (PLAN.md:336-337). The exact set of fields belongs to the settings area.
* `port: null` → assigned from the admin's pool (PLAN.md:333).
* `accept_eula` has to be `true` (PLAN.md:334-335); `eula.txt` comes out of it.

Two further kinds of source, both only from P3 on (PLAN.md:464-465), but already in the contract
because the wizard offers them (`creation-flow-context.ts:321-323`):

```json
{ "kind": "modrinth_modpack", "project_id": "1KVo5zza", "version_id": "8xQZ4rTt" }
```

```json
{ "kind": "mrpack_file", "filename": "Fabulously.Optimized-6.4.0.mrpack" }
```

For `mrpack_file` the request is `multipart/form-data` with the part `payload` (the JSON body
above) and the part `file` (the .mrpack). A two-step route like Modrinth's
(`servers_v0.getReinstallMrpackAuth` + upload to the node,
`vendor/modrinth/api-client/src/modules/archon/servers/v0.ts:138-170`) serves no purpose on one
machine; the browser measures the progress of the upload itself anyway
(`layouts/wrapped/hosting/manage/[id]/onboarding.vue:302-308`).

Response `201` with `Location: /api/v1/servers/<id>` and the complete `Server`:

```json
{
  "server_id": "01J9ZCARQ9V0X2Z4B6D8F1H3JK",
  "name": "Survival",
  "owner_id": "01J9Z0000000000000000MAX01",
  "status": "installing",
  "game": "Minecraft",
  "loader": "Paper",
  "loader_version": "45",
  "mc_version": "1.21.8",
  "net": { "ip": "192.0.2.10", "port": 25567, "domain": "" },
  "ram_mib": 4096,
  "upstream": null,
  "current_user_permissions": ["SERVER_ADMIN"],
  "created_at": "2026-08-12T16:41:00Z"
}
```

Errors:

| Status | `error` | When |
|---|---|---|
| 400 | `invalid_name` | empty or longer than 64 characters |
| 400 | `eula_not_accepted` | `accept_eula` is missing or `false` |
| 400 | `unknown_loader` | the loader is not in the catalog from 2.7 |
| 400 | `unknown_version` | the source does not have this combination of loader/game version/build |
| 403 | `forbidden` | a non-admin sets `owner_id`, or a port outside the pool |
| 404 | `not_found` | `owner_id` points at no user |
| 409 | `budget_exceeded` | the sum of the `ram_mib` of their servers plus the new one > the user's limit (PLAN.md:320-322) |
| 409 | `port_unavailable` | the requested port is taken or the pool is exhausted |
| 413 | `payload_too_large` | .mrpack above the configured limit |
| 502 | `upstream_unavailable` | Mojang/PaperMC/Fabric not reachable |

### 2.3 `GET /api/v1/servers/:id` — a single fetch

Permission: `BASE_READ`. Response `200`, body like an element from 2.1.
Errors: `401 unauthenticated`, `403 forbidden`, `404 not_found`.

`404` also when the server exists but the signed-in user has no right to it whatsoever — otherwise
the split between 404 and 403 gives away other people's server IDs.

### 2.4 `PATCH /api/v1/servers/:id` — renaming

Permission: `ADVANCED`. That is how the interface does it: `general.vue:358` aborts the save
without `canUseAdvancedSettings`, and `canUseAdvancedSettings` is the `ADVANCED` bit
(`server-permissions.ts:97`).

*A borderline case with the settings area:* today the only caller is `general.vue:363`
(`servers_v0.updateName`). But the name belongs to the inventory object, so the endpoint stands
here. If `docs/api/settings.md` describes it as well, this document wins.

**Careful, today this single caller cannot be reached.** `saveGeneral` bails out in its
very first line when the subdomain is invalid (`general.vue:357`), and invalid means, among other
things, "shorter than five characters" (`general.vue:183,187`). Because we fix `net.domain` at
`""`, the input field stands empty, `isValidSubdomain` is permanently `false`, and `updateName`
in `:363` is never reached. The settings area has to remove the subdomain block from `general.vue`,
the same way it removes the SFTP tiles from `advanced.vue`. In detail in 5.3.

Request:

```json
{ "name": "Survival 2" }
```

Response `200`: the complete `Server` object. On top of that a WS `server` message goes to
everyone connected.
Errors: `400 invalid_name`, `401 unauthenticated`, `403 forbidden`, `404 not_found`.

### 2.5 `DELETE /api/v1/servers/:id` — deleting

Permission: the owner (`owner_id == session.user_id`) or a panel admin. Deliberately **no**
server bit: Modrinth knows no deleting (there a server ends with the subscription), and an
editor must not destroy the owner's work.

Precondition: `power_state === "stopped"` or `"crashed"`. Otherwise `409 server_running` — no
silent killing. The check and the deletion run under the same per-server lock as
the power requests (2.6); otherwise a `start` pushes in between the two and the supervisor
outlives its record.

Response `204` with no body. The record is gone at once, the port falls back into the pool, the
owner's budget is free again immediately. Deleting the directory then runs in the
background; with several tens of thousands of files it takes seconds to minutes and must not hold
up the response. Open WebSockets of this server are closed with `4404` (3.4).

Errors: `401 unauthenticated`, `403 forbidden`, `404 not_found`, `409 server_running`,
`409 server_installing` (during a running installation, let it end first).

### 2.6 `POST /api/v1/servers/:id/power` — power

Permission: `POWER_ACTIONS` (`use-server-power-action.ts:19` in combination with
`composables/server-permissions.ts:92`).

Request:

```json
{ "action": "start" }
```

`action` ∈ `start | stop | restart | kill`. Modrinth sends the same four values capitalized
(`use-server-power-action.ts:11`, `servers/v0.ts:104-113`); our adapter lowercases them.

Response `202`:

```json
{ "power_state": "starting", "target": "start" }
```

The response is nothing but the acknowledgment that the request arrived. What is binding is the
WS `state` message that goes to everyone connected right afterwards — including the one who
triggered it.

**States and permitted transitions.** Modrinth's `PowerState` has five values
(`types.ts:1067`), its `FlattenedPowerState` five different ones (`types.ts:1158`), and the client
converts one into the other (`server-manage-core-runtime.ts:68-81`). We send the five final
values right away and save the conversion. Installing is **not** a power state but
`status` — that is how the interface treats it too (`use-server-power-action.ts:21-30`).

```
on request:
  start    stopped | crashed              → starting → running
  stop     running | starting             → stopping → stopped
  restart  running | starting             → stopping → starting → running
  kill     starting | running | stopping  → stopped

on its own, when the process ends:
  exit code 0                             → stopped
  exit code ≠ 0 or an OOM kill            → crashed
```

| Action | Allowed from | Forbidden from | Effect |
|---|---|---|---|
| `start` | `stopped`, `crashed` | `starting`, `running`, `stopping` | start the process through the helper (PLAN.md:189) |
| `stop` | `running`, `starting` | `stopped`, `crashed`, `stopping` | `stop` on stdin, SIGTERM after `stop_grace_seconds`, SIGKILL 10 s later |
| `restart` | `running`, `starting` | `stopped`, `crashed`, `stopping` | `stop`, then `start`; `target` stays `restart` until the end |
| `kill` | `starting`, `running`, `stopping` | `stopped`, `crashed` | SIGTERM to the game's process group, SIGKILL 10 s later — the same grace period as behind `stop` (`docs/api/CONTRACT.md` 4.6) |

The interface already keeps to this: the primary button is locked at `stopping`
(`PanelServerActionButton.vue:44-59`), and kill is enabled at exactly `starting|running|stopping`
(`use-server-power-action.ts:56-60`). You must not rely on that: two browser windows
or a double click are enough. The server therefore holds **one lock per server**, under which the
state is read, the transition is checked and the request is handed to the supervisor. Without it
the check would let two `start`s through and two Java processes would come up on the same port.
Deleting (2.5) takes the same lock.

After a `kill` we report `stopped`, not `crashed` — even when it took the SIGKILL to end the
process. The reason: `powerState === 'crashed'` starts the crash analysis against mclo.gs
(`layouts/wrapped/hosting/manage/overview.vue:164-177`). For a kill you triggered yourself that
would be wrong. For the same reason `oom_killed` stays `false` on a kill, although the
process died from SIGKILL (5.4 and `docs/api/CONTRACT.md` 13.4).

Because SIGTERM comes first, `kill` is not an immediate cut: a server that listens to its signals
saves the world and is gone in a few seconds; only one that ignores SIGTERM sits out the
10 s. The signal goes to the process group, so to whatever the server started itself as well. The
binding description, restrictions included, is in `docs/api/CONTRACT.md` 4.6.

Errors:

| Status | `error` | When |
|---|---|---|
| 401 | `unauthenticated` | no session |
| 403 | `forbidden` | no `POWER_ACTIONS` |
| 404 | `not_found` | unknown or invisible |
| 409 | `invalid_power_transition` | the transition is not allowed by the table; `message` names the current and the requested state |
| 409 | `server_installing` | `status === "installing"` |
| 409 | `server_broken` | `status === "broken"`, on `start` |
| 409 | `budget_exceeded` | the admin lowered the limit below what is already handed out; running servers keep running, stopped ones no longer start (PLAN.md:364-366) |

### 2.7 The loader catalog for the wizard

Two small endpoints, without which the creation step "What do you want to play" does not work.

`GET /api/v1/loaders` — permission: session. It fills the `availableLoaders` property of
`CreationFlowModal` (`components/flows/creation-flow-modal/index.vue:29,46,72`), a plain
`string[]` with no closed enumeration — our ten (PLAN.md:377-400) instead of the four preset ones.
`formatLoaderLabel` labels anything unknown by capitalizing the first letter
(`utils/loaders.ts:18`), and that carries for all ten.

**But it reaches only our own creation page.** The second place where the same wizard
opens — `ServerSetupModal` behind "Reinstall" and behind the loader and
game version labels — carries its seven loaders as a local constant in the vendor code
(`ServerSetupModal.vue:65`, bound in `:5`); there is neither a property nor an
injection there through which we could replace them. So Folia, Leaf and Velocity can be chosen
when creating a server and not when reinstalling one. **For the settings area to decide:**
patch that one line in the vendor code or live with the difference. (The third occurrence,
`onboarding.vue:66`, lies in `layouts/wrapped/` and went away with it.)

```json
{
  "loaders": [
    { "id": "vanilla",  "name": "Vanilla",  "kind": "server", "needs_installer": false },
    { "id": "paper",    "name": "Paper",    "kind": "server", "needs_installer": false },
    { "id": "folia",    "name": "Folia",    "kind": "server", "needs_installer": false },
    { "id": "purpur",   "name": "Purpur",   "kind": "server", "needs_installer": false },
    { "id": "leaf",     "name": "Leaf",     "kind": "server", "needs_installer": false },
    { "id": "fabric",   "name": "Fabric",   "kind": "server", "needs_installer": false },
    { "id": "velocity", "name": "Velocity", "kind": "proxy",  "needs_installer": false },
    { "id": "neoforge", "name": "NeoForge", "kind": "server", "needs_installer": true },
    { "id": "quilt",    "name": "Quilt",    "kind": "server", "needs_installer": true },
    { "id": "forge",    "name": "Forge",    "kind": "server", "needs_installer": true }
  ]
}
```

`GET /api/v1/loaders/:loader/versions` — permission: session. Answers in the structure
`CreationFlowOptions.getLoaderManifest` expects (`creation-flow-context.ts:217,237`, type
`LauncherMeta.Manifest.v0.Manifest`,
`vendor/modrinth/api-client/src/modules/launcher-meta/types.ts:22-25`) — in
snake_case, though; the adapter does the five lines of renaming, so that our API stays uniform.

```json
{
  "game_versions": [
    {
      "id": "1.21.8",
      "stable": true,
      "loaders": [
        { "id": "45", "url": "", "stable": true },
        { "id": "44", "url": "", "stable": true }
      ]
    },
    {
      "id": "1.21.7",
      "stable": true,
      "loaders": [
        { "id": "38", "url": "", "stable": true }
      ]
    }
  ]
}
```

`url` stays empty: the client downloads nothing itself, the server does that. Errors:
`404 unknown_loader`, `502 upstream_unavailable`.

**The path name is not our `id` from the catalog.** The wizard maps it first:
`neoforge` becomes `neo` (`creation-flow-context.ts:350-352`, the same function once more in
`CustomSetupStage.vue:331-333`), and the cache runs under that name too
(`creation-flow-context.ts:34-35,356,366`). Our endpoint has to accept `neo`; it may accept
`neoforge` as well, but it will never be asked for it. The other nine arrive unchanged.

Three of the ten never reach it anyway:

| Loader | Why not |
|---|---|
| `vanilla` | `fetchLoaderMetadata` turns back at once (`creation-flow-context.ts:407`); the game versions then come unfiltered from the tags provider |
| `paper` | hard-wired against `fill.papermc.io` (`CustomSetupStage.vue:432`, base URL in `vendor/modrinth/api-client/src/modules/paper/v3.ts:6`) |
| `purpur` | hard-wired against `api.purpurmc.org` (`CustomSetupStage.vue:449`, `modules/purpur/v2.ts:6`) |

These two base URLs cannot be redirected: `types/client.ts:43-57` knows only
`labrinthBaseUrl`, `archonBaseUrl` and `sharedInstancesBaseUrl`. That leaves the seven for which
`getLoaderManifest` really is asked — Fabric, Folia, Leaf, Velocity, `neo`, Quilt, Forge —,
and those are exactly the ones we need, because Modrinth's launcher-meta knows neither Folia nor
Leaf nor Velocity.

**The `id` of a `game_version` has to be a Minecraft version.** The list shown is the
intersection of the tags provider (`CustomSetupStage.vue:293,346`, `providers/tags.ts:6-9`, so
from the real api.modrinth.com) and the `id`s from our response (`CustomSetupStage.vue:344-381`).
What is in both is offered; what is only in ours disappears. For Folia and Leaf that fits,
they are ordered by game version. **Not for Velocity:** a proxy has no
game version, and PaperMC lists it under versions like `3.4.0-SNAPSHOT`. If we passed those
through, the selection field would stay empty and Velocity could not be created despite its
catalog entry. For Velocity we therefore hand out **all** the game versions the tags provider
carries, and hang the same build list on each of them — when creating a proxy the game version is
a field without effect, we only remember it for the display. The build list itself does not have
to be repeated ten times: one row with the `id` `${modrinth.gameVersion}` holds it once, and the
remaining rows may then carry `loaders: []` (`CustomSetupStage.vue:369-378,472-479`). That is the
road Fabric takes. Even it does not save the enumeration of the game versions themselves — `:474`
still demands one row per version.

### 2.8 The WebSocket

`GET /api/v1/servers/:id/ws` — an upgrade. Permission: `BASE_READ`. No fetching a token
up front; Modrinth's two-step route (`GET /servers/:id/ws` → JWT → `{event:'auth',jwt}`,
`servers/v0.ts:92-98`, `types.ts:1188-1191`) drops out, because the cookie already travels with
the upgrade. More in section 3.

That the cookie travels along is also the catch: no same-origin rule applies to WebSocket
upgrades, so a foreign window can open the connection with our session cookie.
The upgrade therefore checks the `Origin` header against the panel's configured address and
closes with `4403` when it is missing or differs — the same check the cookie route gets for free
from `SameSite` on the REST calls.

---

## 3. WebSocket messages

One socket per server. Envelope `{ "type": "...", … }`. All times RFC 3339 UTC.

### 3.1 Server → client, my area

**`state`** — on connect and on every change. Corresponds to Modrinth's `WSStateEvent`
(`types.ts:1173-1183`), without its `debug` and without the intermediate layer
`FlattenedPowerState`.

```json
{
  "type": "state",
  "power_state": "running",
  "target": null,
  "uptime_seconds": 3812,
  "exit_code": null,
  "oom_killed": false,
  "install": null,
  "install_error": null
}
```

During an installation:

```json
{
  "type": "state",
  "power_state": "stopped",
  "target": null,
  "uptime_seconds": 0,
  "exit_code": null,
  "oom_killed": false,
  "install": { "phase": "downloading", "percent": 62, "started_at": "2026-08-12T16:41:02Z" },
  "install_error": null
}
```

After a failed attempt:

```json
{
  "type": "state",
  "power_state": "stopped",
  "target": null,
  "uptime_seconds": 0,
  "exit_code": null,
  "oom_killed": false,
  "install": null,
  "install_error": { "step": "modloader", "description": "the specified version may be incorrect" }
}
```

`install.percent` is 0–100 (`InstallingBanner.vue:201` divides by 100). If the download size is
unknown, we send `0`; the display then goes into its indeterminate state
(`InstallingBanner.vue:204-206`). Our phases and how they map onto Modrinth's four
(`types.ts:1160`):

| Our `phase` | Modrinth's `SyncInstallPhase` | Display text |
|---|---|---|
| `resolving` | `Analyzing` | — the banner is hidden in this phase (`ServerPanelAdmonitions.vue:179`), so keep it short |
| `downloading` | `InstallingLoader` | "Installing platform…" |
| `installing` | `InstallingLoader` | ditto — the `--installServer` run for NeoForge/Quilt/Forge (PLAN.md:394-400) |
| `modpack` | `InstallingPack` | "Installing modpack…" |
| `addons` | `Addons` | "Installing addons…" — belongs to the content area |

`resolving` **never** appears when creating a server: there the resolving is done before the 201
(2.2), and while it runs there is neither a record nor a socket. The phase is in the contract for
reinstalling and repairing an existing server, where the same step runs with the socket
open.

`install_error.step` and `.description` are mapped onto fixed texts in the display
(`InstallingBanner.vue:150-176`); we use the strings recognized there literally (`modloader` +
"the specified version may be incorrect" / "this version is not yet supported" / "internal error",
`modpack` + "no primary file" / "failed to install"), everything else falls back to
`description` as plain text.

**`stats`** — for the interval see 3.2.

```json
{
  "type": "stats",
  "cpu_percent": 34.7,
  "ram_usage_bytes": 2684354560,
  "ram_total_bytes": 4294967296,
  "storage_usage_bytes": 1932735283,
  "storage_total_bytes": 500107862016
}
```

**`server`** — the inventory object has changed (name, port, status, loader after a
finished installation). Replaces reloading over REST.

```json
{
  "type": "server",
  "server": {
    "server_id": "01J9ZCARQ9V0X2Z4B6D8F1H3JK",
    "name": "Survival",
    "owner_id": "01J9Z0000000000000000MAX01",
    "status": "available",
    "game": "Minecraft",
    "loader": "Paper",
    "loader_version": "45",
    "mc_version": "1.21.8",
    "net": { "ip": "192.0.2.10", "port": 25567, "domain": "" },
    "ram_mib": 4096,
    "upstream": null,
    "current_user_permissions": ["SERVER_ADMIN"],
    "created_at": "2026-08-12T16:41:00Z"
  }
}
```

`current_user_permissions` is always filled from the recipient's point of view.

**`error`** — a message from the client was rejected. The same codes as HTTP.

```json
{ "type": "error", "error": "forbidden", "message": "missing permission EXEC_COMMANDS" }
```

### 3.2 Interval and order

On connect, in this order:

1. one `server` message,
2. one `state` message,
3. up to ten `stats` messages from the ring buffer, oldest first (so that the graph starts out
   filled, see 1.4) — **only when the server is running.** If it stands still, the look back
   would be a lie: the graph would show the last ten seconds before the stop as though they were
   now. Then only a single fresh sample goes out, and the watchdog fills the rest with zeros,
4. after that whatever the other areas send on connect (the console backlog).

Ongoing:

* `stats` every second while `power_state === "running"`. Below the 5 s threshold of the
  client watchdog (`server-manage-core-runtime.ts:65`) and a fit for the ten-point graph, which
  then shows ten seconds.
* With a server that is not running: one sample every 30 s only, because `storage_usage_bytes`
  keeps changing through file operations. The once-a-second interval is not needed — in the
  meantime the watchdog pulls CPU and memory to zero on its own and keeps the storage value
  (`server-manage-core-runtime.ts:177-183`).
* `state` on change only, plus at most once a second during an installation
  (progress).
* `storage_usage_bytes` is not measured afresh for every sample — a `du` over a
  modpack directory is expensive. A background run every 30 s fills a cache, out of
  which every `stats` message is served.
* a WebSocket ping every 30 s at the protocol level; no message type of its own for it.

### 3.3 Client → server

In my area: **none**. Power runs over REST (2.6), so that failures have an HTTP status
and the interface keeps its usual error handling
(`use-server-power-action.ts:74-85`). The only incoming message on the whole socket is
`command` from the console area (`docs/api/console.md`).

### 3.4 Close codes

| Code | Meaning | Effect in the provider |
|---|---|---|
| 1000 | normal, the page was left | `isConnected = false` |
| 4401 | no session or an expired one | `isWsAuthIncorrect = true` |
| 4403 | session valid, but no `BASE_READ` | `isWsAuthIncorrect = true` |
| 4404 | the server no longer exists (deleted) | the provider cleans up and sends you to the list |
| 4429 | too many sockets for the same session and the same server (limit 4) | the provider does not reconnect |

The upgrade is **accepted and then closed**, not rejected with HTTP 401. The reason: on a
rejected upgrade a browser gets nothing but a bare `error` event without a status, and
`isWsAuthIncorrect` could not be told apart from "the network is gone". That is exactly what
Modrinth has the `auth-incorrect` message for (`types.ts:1080-1082`); the close code does the same
thing without an extra pair of messages.

### 3.5 Messages from other areas on the same socket

For the overview only; the other document is binding in each case: `log` and `log4j` (console),
`fs_ops` (files), `backup_progress` (backups), `content_installed` (content). All
areas share this one socket; nobody opens a second one.

---

## 4. Data types

```ts
// ---------- Inventory ----------

export type ServerStatus = 'installing' | 'available' | 'broken'

/** Wider than `Archon.Servers.v0.Loader` (`types.ts:590-598`); the adapter casts. */
export type ServerLoader =
  | 'Vanilla' | 'Paper' | 'Folia' | 'Purpur' | 'Leaf'
  | 'Fabric' | 'Velocity' | 'NeoForge' | 'Quilt' | 'Forge'

export type ServerPermission =
  | 'BASE_READ' | 'POWER_ACTIONS' | 'EXEC_COMMANDS' | 'FILES_WRITE' | 'SETUP'
  | 'BACKUPS' | 'ADVANCED' | 'RESET_SERVER' | 'MANAGE_USERS' | 'SERVER_ADMIN'

export interface ServerNet {
  ip: string | null // from the configuration, see 4.2
  port: number
  domain: string // always "", see 5.3
}

export interface ServerUpstream {
  kind: 'modpack'
  project_id: string
  version_id: string
}

export interface Server {
  server_id: string
  name: string
  owner_id: string
  status: ServerStatus
  game: 'Minecraft'
  loader: ServerLoader | null
  loader_version: string | null
  mc_version: string | null
  net: ServerNet
  ram_mib: number
  upstream: ServerUpstream | null
  current_user_permissions: ServerPermission[]
  created_at: string
}

export interface ServerOwner {
  id: string
  username: string
  avatar_url: string | null
}

export interface ServerListResponse {
  servers: Server[]
  users: Record<string, ServerOwner>
}

// ---------- Creating ----------

export type CreateServerSource =
  | { kind: 'loader'; loader: string; loader_version: string | null; mc_version: string }
  | { kind: 'modrinth_modpack'; project_id: string; version_id: string }
  | { kind: 'mrpack_file'; filename: string }

export interface CreateServerRequest {
  name: string
  source: CreateServerSource
  ram_mib: number
  port: number | null
  owner_id: string | null
  accept_eula: boolean
  properties: PropertiesFields | null // Archon.Content.v1.PropertiesFields
}

export interface UpdateServerRequest {
  name?: string
}

// ---------- Power ----------

export type PowerAction = 'start' | 'stop' | 'restart' | 'kill'
export type PowerState = 'stopped' | 'starting' | 'running' | 'stopping' | 'crashed'

/**
 * As at Modrinth (`types.ts:1179`): `kill` never stays as a target — there is nothing left to
 * wait for afterwards, even when the end takes up to 10 s (2.6).
 * No reader in `layouts/shared/` or `components/servers/`; we carry it because otherwise
 * nobody would see that a `stopping` belongs to a restart.
 */
export type PowerTarget = 'start' | 'stop' | 'restart'

export interface PowerRequest { action: PowerAction }

export interface PowerResponse {
  power_state: PowerState
  target: PowerTarget | null
}

// ---------- Loader catalog ----------

export interface LoaderInfo {
  id: string
  name: string
  kind: 'server' | 'proxy'
  needs_installer: boolean
}

export interface LoaderListResponse { loaders: LoaderInfo[] }

export interface LoaderBuild { id: string; url: string; stable: boolean }

export interface LoaderGameVersion {
  id: string
  stable: boolean
  loaders: LoaderBuild[]
}

export interface LoaderVersionsResponse { game_versions: LoaderGameVersion[] }

// ---------- WebSocket ----------

export type InstallPhase = 'resolving' | 'downloading' | 'installing' | 'modpack' | 'addons'

export interface InstallProgress {
  phase: InstallPhase
  percent: number // 0..100
  started_at: string
}

export interface InstallError {
  step: string
  description: string
}

export interface WsStateMessage {
  type: 'state'
  power_state: PowerState
  target: PowerTarget | null
  uptime_seconds: number
  exit_code: number | null
  oom_killed: boolean
  install: InstallProgress | null
  install_error: InstallError | null
}

export interface WsStatsMessage {
  type: 'stats'
  cpu_percent: number
  ram_usage_bytes: number
  ram_total_bytes: number
  storage_usage_bytes: number
  storage_total_bytes: number
}

export interface WsServerMessage {
  type: 'server'
  server: Server
}

export interface WsErrorMessage {
  type: 'error'
  error: string
  message: string
}

export type WsServerAreaMessage =
  | WsStateMessage
  | WsStatsMessage
  | WsServerMessage
  | WsErrorMessage

// ---------- Errors ----------

export interface ApiErrorBody {
  error: string
  message: string
}
```

### 4.1 Where the metrics come from

| Field | Where exactly it comes from |
|---|---|
| `cpu_percent` | the increase of `utime+stime` over all processes of the server's tree from `/proc/<pid>/stat`, divided by the elapsed wall-clock time **times the owner's CPU quota** (`cpu.max` of the user cgroup, PLAN.md:259), times 100. 100 % therefore means "this server alone uses up its owner's budget". If the user has no quota, the denominator is the machine's core count. |
| `ram_usage_bytes` | the sum of the RSS over the process tree from `/proc/<pid>/statm`. **Not** from the cgroup: the cgroup is created per user, not per server (PLAN.md:229-234), and "limits per server" are explicitly postponed (PLAN.md:502). A `memory.current` would give the sum of all of this user's servers. |
| `ram_total_bytes` | `Server.ram_mib × 1048576` — that is, exactly this server's `-Xmx`, set when it was created, out of the user's budget (PLAN.md:256, 332). No plan, no machine size. That makes the percentage in the tile "share of your own heap limit", and that is the number that concerns the user. |
| `storage_usage_bytes` | the size of the server directory `…/users/<uid>/servers/<sid>/`, measured again in the background every 30 s. |
| `storage_total_bytes` | `statvfs` of the file system the server directory lies on (`f_blocks × f_frsize`). There is **no** disk quota here — PLAN.md:229-231 names only `memory.*`, `cpu.max`, `pids.max`. The value is therefore machine-wide and deliberately not a promise; it is also the only field of `ServerStatsSample` that no component displays (1.4). |

That `ram_usage_bytes` (RSS, includes off-heap and metaspace) can rise above `ram_total_bytes`
(`-Xmx`) is normal and not an error: the graph caps at 100 (`ServerManageStats.vue:125`),
the tile shows the real value. To force a number below 100 % you would have to measure the
JVM's heap use, and that would need JMX in the server process — we refuse that.

Summing the RSS over a process tree counts shared pages more than once. For a
Minecraft server that is of no consequence, because a single JVM process holds practically
everything there; with the loaders that use a start script (NeoForge, Forge) a shell hangs next to
it that weighs nothing.

### 4.2 Where `net.ip` comes from

The only field of the inventory object the machine does not know by itself. Here a server binds
on `0.0.0.0`; there is no address per server. The value is displayed all the same
(`server-settings/pages/network.vue:248`), and people need it in order to connect.

So it comes from the configuration: a field `public_address` in `config.toml`, which the
installer fills on its first run with whatever it finds, and which the admin can overwrite.
It may be a name instead of an address — `ServerNet.ip` is a string, not an address type
(`types.ts:633-637`), and `network.vue:248` prints it raw. If nothing is set, we send
`null`; the field is explicitly `string | null`.

What we do **not** do: guess the address at runtime. Neither the machine's first non-loopback
address nor a call back to a foreign service would give the right thing behind NAT or a reverse
proxy, and either would be a line you never get rid of again.

---

## 5. Open questions and assumptions

### 5.1 Decided, with reasoning

1. **Creating is synchronous up to the checks, an operation after that.** In detail in 2.2. The
   alternative — an operation object with an ID of its own and `GET /operations/:id` — would be a
   second progress mechanism next to the one the interface already has.
2. **No `busy_reasons` in the API.** The interface computes it itself; we deliver `status`
   and `state.install` and nothing else (1.5).
3. **No pagination, no server-side search** in `GET /servers` (2.1).
4. **Permissions as a list of names** instead of as a number (1.3).
5. **Deleting demands a stopped server** (2.5). No `?force=true`: whoever wants to kill
   presses Kill first — that is one click and it makes the intent visible.
6. **`kill` leads to `stopped`, not to `crashed`** (2.6).
7. **Close codes instead of an `auth-incorrect` message** (3.4).
8. **No progress display in the server list**, only `status: installing` and a 5 s poll
   (2.1). A list-wide socket would contradict the ruling "one socket per server".
9. **`created_at` is new** compared with Modrinth's `Server`. We need it because `flows.intro` is
   constantly `false` here and the list would otherwise have no criterion for showing a freshly
   created server at the top.
10. **The loader catalog is ours** (2.7), even though the vendor component goes past it for
    Vanilla, Paper and Purpur. There is no other way to make Folia, Leaf and Velocity
    selectable — and when reinstalling they stay unselectable all the same, as long as
    `ServerSetupModal.vue:65` is not touched.
11. **`net.ip` comes from the configuration**, not from runtime detection (4.2).
12. **Check and act run under one lock**: budget and port in one transaction (2.2),
    power transitions and deleting under a per-server lock (2.5, 2.6). Without that, every
    state check in this document is nothing but a recommendation.
13. **The WebSocket checks `Origin`** (2.8). A cookie alone is not enough on an upgrade.

### 5.2 `world_id`: the constant `"default"`

The contract demands `worldId: Ref<string | null>` (`server-context.ts:39`), and shared
components pass it on to their calls with `!` — `installContent(serverId, worldId.value!, …)`
(`onboarding.vue:353`), `backups_queue_v1.ackCreate(serverId, worldId.value!, …)`
(`ServerPanelAdmonitions.vue:295-299`), and also as part of localStorage keys
(`layouts/shared/browse-tab/composables/install-logic.ts:80-82`). So `null` is forbidden in
practice.

The proposal: the constant `"default"`. Our paths carry **no** world segment; the
adapter functions of the other areas ignore the argument. A random ULID would be
worse, because it lands in localStorage keys and would orphan the stored states after the
database is rebuilt.

### 5.3 `net.domain` is empty — and costs a change to vendor code

`ServerSubdomainLabel.vue:18` hard-appends `.modrinth.gg` to the value. A sensible value from
us would therefore appear as "192.0.2.10:25565.modrinth.gg". Since `ServerInfoLabels` renders the
label only when `domain` is not empty (`ServerInfoLabels.vue:24`), `""` is the only value that
leaves this component unchanged. We show the server address in our own list and in our own
server header. **For the frontend area to decide:** whether an address label of our own is placed
next to `ServerInfoLabels` instead.

But a second component reads the same value not as a label but as an **input field with a
required check**, and there the decision tips over:

```
general.vue:175   serverSubdomain = data.net?.domain ?? ''      →  ''
general.vue:183   isValidLengthSubdomain = length >= 5          →  false
general.vue:187   isValidSubdomain = length && characters       →  false
general.vue:357   if (!isValidServerName || !isValidSubdomain) return
general.vue:363   updateName(...)                               ←  never reached
```

With `domain: ""`, **renaming is impossible**, even though the save bar appears
(`general.vue:138` checks the name only) — the button simply does nothing. On top of that a
red line "Subdomain must be at least 5 characters long." stands on the page permanently
(`general.vue:64-67`).

There is no dodging it: a `domain` with five characters makes the check happy, but it brings
the `.modrinth.gg` label back, and with it a write path to
`servers_v0.changeSubdomain` (`general.vue:381`) that does not exist here. Both values are
wrong, so the problem is not the value but the block.

**The ruling:** `net.domain` stays `""`, and the settings area removes the subdomain field
from `general.vue`, along with its check and its write path — the same operation
`advanced.vue` needs for SFTP anyway. This is the first place where we change vendor code
instead of only filling it; it belongs in the change log, because every update of
`packages/ui` overwrites it again.

### 5.4 Gaps I name instead of keeping quiet about

* **`serverFull` stays `null`.** Should a later area hang in a shared component that does read
  it, this contract has to be amended. What has been checked is only today's
  stock of `layouts/shared/` and `components/servers/`.
* **The server icon is in none of my endpoints.** Modrinth's deleted
  `ServerListing.vue` fetched it itself through the injected client —
  `archon.servers_v0.getFilesystemAuth` and
  `kyros.files_v0.downloadFileWithAuth('/server-icon.png')` (`ServerListing.vue:563-566`), with a
  fallback to `/server-icon-original.png` and to the modpack icon, which it then even uploaded
  (`:587-605`). Since the component is our own work now anyway (PLAN.md:75), an endpoint in the
  files area that serves `/server-icon.png` is enough. **For the files area to decide**;
  I create no field in `Server` for it.
* **Player count.** The deleted `ServerListing` had the properties `online` and
  `playerCount` (`:429-433`) and Modrinth filled them nowhere (`index.vue:181-186` binds
  `v-bind="server"` plus four billing fields, and `Server` has neither of those two). The
  component that stays with us wants it differently: `ServerInfoLabels.vue:3-8` passes
  `serverData.players.current`, `serverData.players.max` and `serverData.online` on to
  `ServerPlayerCount` — a field `players`, not `playerCount`. Whoever sets `show-player-count`
  without passing `players` along accesses `undefined`. It could be filled through a
  server list ping. It is not in the plan, so it is not in the contract — the place for it is
  there, and with that the shape is fixed too.
* **`install_error` and the retry button.** The button in the installation banner triggers
  `content_v1.repair(serverId, worldId)` (`root.vue:1083-1099`). The matching endpoint therefore
  belongs to the content area. For a pure loader server without a modpack, though, "repair"
  is the same as "download the jar again" — **to be settled between the content area and the
  settings area**: who provides this endpoint. I deliberately do not create it here.
* **`oom_killed` is a guess.** The cgroup belongs to the user, not to the server
  (PLAN.md:229-234); all we can establish is that the counter `oom_kill` in
  `memory.events` went up while this process ended through SIGKILL. With two
  servers of the same user dying at the same time, the attribution can be off. Plus the
  condition without which the field would be useless: it holds only when the SIGKILL did **not**
  come from the supervisor itself. The counter never goes down again, so otherwise, after the
  first real out-of-memory, every kill on this account would have reported an out-of-memory
  forever (`docs/api/CONTRACT.md` 13.4).
* **Cancelling a running installation.** There is no endpoint for it. Modrinth has none,
  and `DELETE` refuses while it runs (`409 server_installing`). Whoever wants to stop the download
  has to wait until it runs into `broken`. **Somebody should decide** whether a
  `POST /servers/:id/install/cancel` gets added.
* **`stop_grace_seconds` is decided** and no longer a gap: the value sits in
  `panel_settings` with a default of 60 s (`docs/api/CONTRACT.md` 12.10), and the ladder behind it
  runs `stop` → grace period → SIGTERM → 10 s → SIGKILL (`docs/api/CONTRACT.md` 4.6). With that,
  `stopping` has a way out even when the game never executes the console command.
* **A warning on overbooked memory.** PLAN.md:268-269 demands a warning when the sum of the
  `-Xmx` exceeds the user's budget (allowed, but dangerous). The return value of
  `POST /servers` does not carry it today. **For the user administration area to decide:**
  a field of its own in the response, or a pre-check `GET /users/me/budget`.


